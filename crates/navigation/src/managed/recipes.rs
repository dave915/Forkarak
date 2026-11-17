//! Source-specific resolvers. Every archive has an independently published digest;
//! npm resolves a lock then ci verifies registry SRI with all scripts disabled.
use super::{
    InstallControl, InstallPhase, InstalledTool, ToolArtifact, ToolDistribution, archive, io,
};
use crate::{ServerConfig, languages::RECIPES};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .with_context(|| format!("Official metadata missing {key}"))
}
fn arch() -> Result<&'static str> {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            Ok("arm64")
        } else if cfg!(target_arch = "x86_64") {
            Ok("x64")
        } else {
            bail!("Unsupported macOS architecture")
        }
    } else {
        bail!("Managed runtime catalog currently supports macOS only")
    }
}
fn descriptor(version: &str, url: &str, digest: &str, metadata: &str) -> ToolArtifact {
    ToolArtifact {
        version: version.into(),
        source_url: url.into(),
        checksum: digest.into(),
        metadata_url: metadata.into(),
    }
}
fn github(repo: &str, scratch: &Path, c: &InstallControl) -> Result<Value> {
    io::json(
        &format!("https://api.github.com/repos/{repo}/releases/latest"),
        scratch,
        c,
    )
}
fn github_asset(
    release: &Value,
    name: &str,
    scratch: &Path,
    c: &InstallControl,
) -> Result<ToolArtifact> {
    let a = release["assets"]
        .as_array()
        .context("Missing release assets")?
        .iter()
        .find(|a| a["name"] == name)
        .with_context(|| format!("Official release has no {name} asset"))?;
    let url = str_field(a, "browser_download_url")?;
    let digest = match a["digest"].as_str().filter(|d| d.starts_with("sha256:")) {
        Some(d) => d.into(),
        None => format!(
            "sha256:{}",
            io::checksum(&format!("{url}.sha256"), 64, scratch, c)?
        ),
    };
    Ok(descriptor(
        str_field(release, "tag_name")?,
        url,
        &digest,
        release["url"].as_str().unwrap_or(""),
    ))
}
fn unpack(a: &ToolArtifact, to: &Path, scratch: &Path, c: &InstallControl) -> Result<()> {
    let file = io::artifact(a, scratch, c)?;
    archive::extract(&file, to, c)
}
fn files(root: &Path, predicate: &impl Fn(&Path) -> bool) -> Result<Vec<PathBuf>> {
    fn visit(root: &Path, p: &impl Fn(&Path) -> bool, out: &mut Vec<PathBuf>) -> Result<()> {
        for e in fs::read_dir(root)? {
            let e = e?;
            let path = e.path();
            let ty = e.file_type()?;
            if ty.is_dir() {
                visit(&path, p, out)?;
            } else if ty.is_file() && p(&path) {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    visit(root, predicate, &mut out)?;
    Ok(out)
}
fn find(root: &Path, suffix: &str) -> Result<PathBuf> {
    let matches = files(root, &|p| p.to_string_lossy().ends_with(suffix))?;
    if matches.len() != 1 {
        bail!(
            "Expected one {suffix} in distribution, found {}",
            matches.len()
        );
    }
    Ok(matches[0].clone())
}
fn env_config(
    language: &str,
    program: &Path,
    args: Vec<String>,
    path: &str,
    extra: &[(&str, String)],
) -> ServerConfig {
    let mut env = vec![
        "-i".into(),
        "FORKARAK_MANAGED=1".into(),
        format!("PATH={path}"),
        "HOME={workspace}/home".into(),
        "TMPDIR={workspace}".into(),
        "LANG=en_US.UTF-8".into(),
    ];
    env.extend(extra.iter().map(|(k, v)| format!("{k}={v}")));
    env.push(program.to_string_lossy().into_owned());
    env.extend(args);
    ServerConfig {
        language: language.into(),
        program: "/usr/bin/env".into(),
        args: env,
    }
}
fn node(
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
    artifacts: &mut Vec<ToolArtifact>,
) -> Result<(PathBuf, PathBuf)> {
    let source = "https://nodejs.org/dist/index.json";
    let index = io::json(source, scratch, c)?;
    let release = index
        .as_array()
        .context("Invalid Node release index")?
        .iter()
        .find(|r| {
            r["lts"].is_string() && r["version"].as_str().is_some_and(|v| v.starts_with("v22."))
        })
        .context("No supported Node 22 LTS release")?;
    let version = str_field(release, "version")?;
    let file = format!("node-{version}-darwin-{}.tar.gz", arch()?);
    let base = format!("https://nodejs.org/dist/{version}/");
    let sums = io::text(&format!("{base}SHASUMS256.txt"), scratch, c)?;
    let sum = sums
        .lines()
        .find_map(|line| {
            let mut s = line.split_whitespace();
            let sum = s.next()?;
            (s.next()? == file).then_some(sum)
        })
        .context("Node asset missing in published checksum list")?;
    let a = descriptor(
        version,
        &format!("{base}{file}"),
        &format!("sha256:{sum}"),
        source,
    );
    unpack(&a, &payload.join("node"), scratch, c)?;
    artifacts.push(a);
    Ok((
        find(&payload.join("node"), "/bin/node")?,
        find(&payload.join("node"), "/npm/bin/npm-cli.js")?,
    ))
}
fn java(
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
    artifacts: &mut Vec<ToolArtifact>,
) -> Result<PathBuf> {
    let source = "https://api.github.com/repos/adoptium/temurin25-binaries/releases/latest";
    let release = io::json(source, scratch, c)?;
    let prefix = if arch()? == "arm64" {
        "OpenJDK25U-jdk_aarch64_mac_hotspot_"
    } else {
        "OpenJDK25U-jdk_x64_mac_hotspot_"
    };
    let name = release["assets"]
        .as_array()
        .context("Invalid Temurin release")?
        .iter()
        .find_map(|a| {
            a["name"]
                .as_str()
                .filter(|n| n.starts_with(prefix) && n.ends_with(".tar.gz"))
        })
        .context("No Temurin JDK25 macOS asset")?;
    let asset = release["assets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == name)
        .unwrap();
    let url = str_field(asset, "browser_download_url")?;
    let digest = match asset["digest"]
        .as_str()
        .filter(|s| s.starts_with("sha256:"))
    {
        Some(d) => d.into(),
        None => format!(
            "sha256:{}",
            io::checksum(&format!("{url}.sha256.txt"), 64, scratch, c)?
        ),
    };
    let a = descriptor(str_field(&release, "tag_name")?, url, &digest, source);
    unpack(&a, &payload.join("java"), scratch, c)?;
    artifacts.push(a);
    find(&payload.join("java"), "/Contents/Home/bin/java")
}
fn maven(
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
    artifacts: &mut Vec<ToolArtifact>,
) -> Result<PathBuf> {
    let base = "https://repo.maven.apache.org/maven2/org/apache/maven/apache-maven/";
    let metadata = format!("{base}maven-metadata.xml");
    let text = io::text(&metadata, scratch, c)?;
    let xml = roxmltree::Document::parse(&text)?;
    let version = xml
        .descendants()
        .filter(|n| n.has_tag_name("version"))
        .filter_map(|n| n.text())
        .filter(|s| s.starts_with("3.") && s.split('.').all(|p| p.parse::<u32>().is_ok()))
        .max_by_key(|v| {
            v.split('.')
                .map(|s| s.parse::<u32>().unwrap())
                .collect::<Vec<_>>()
        })
        .context("No stable Maven 3 release")?;
    let url = format!("{base}{version}/apache-maven-{version}-bin.tar.gz");
    let sum = io::checksum(&format!("{url}.sha512"), 128, scratch, c)?;
    let a = descriptor(version, &url, &format!("sha512:{sum}"), &metadata);
    unpack(&a, &payload.join("maven"), scratch, c)?;
    artifacts.push(a);
    find(&payload.join("maven"), "/bin/mvn")
}
fn npm(
    package: &str,
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
    artifacts: &mut Vec<ToolArtifact>,
) -> Result<(String, Vec<ServerConfig>)> {
    let recipe = RECIPES.iter().find(|r| r.id == package).unwrap();
    let (node, npm) = node(payload, scratch, c, artifacts)?;
    let project = payload.join("server");
    fs::create_dir(&project)?;
    let mut dependencies = serde_json::Map::new();
    let mut primary = String::new();
    for name in recipe.npm {
        let encoded = name.replace('/', "%2f");
        let version = if package == "vue" && *name == "@vue/language-server" {
            "2.2.12"
        } else if *name == "typescript" {
            "5.9.3"
        } else {
            "latest"
        };
        let metadata = format!("https://registry.npmjs.org/{encoded}/{version}");
        let meta = io::json(&metadata, scratch, c)?;
        let exact = str_field(&meta, "version")?;
        let integrity = meta["dist"]["integrity"]
            .as_str()
            .context("npm package lacks registry SRI integrity")?;
        let url = meta["dist"]["tarball"]
            .as_str()
            .context("npm package lacks tarball URL")?;
        artifacts.push(descriptor(exact, url, integrity, &metadata));
        dependencies.insert((*name).into(), json!(exact));
        if primary.is_empty() {
            primary = exact.into();
        }
    }
    fs::write(
        project.join("package.json"),
        serde_json::to_vec_pretty(
            &json!({"name":"forkarak-managed-server","version":"1.0.0","private":true,"dependencies":dependencies}),
        )?,
    )?;
    let global_config = scratch.join("npm-globalrc");
    fs::write(&global_config, b"ignore-scripts=true\n")?;
    let config = scratch.join("npmrc");
    fs::write(
        &config,
        b"ignore-scripts=true\naudit=false\nfund=false\nregistry=https://registry.npmjs.org/\n",
    )?;
    let bin = node.parent().unwrap().to_string_lossy();
    let env = vec![
        ("PATH".into(), format!("{bin}:/usr/bin:/bin")),
        (
            "NPM_CONFIG_USERCONFIG".into(),
            config.to_string_lossy().into_owned(),
        ),
        (
            "NPM_CONFIG_GLOBALCONFIG".into(),
            global_config.to_string_lossy().into_owned(),
        ),
        (
            "NPM_CONFIG_CACHE".into(),
            scratch.join("npm-cache").to_string_lossy().into_owned(),
        ),
        ("NPM_CONFIG_UPDATE_NOTIFIER".into(), "false".into()),
    ];
    let common = [
        "--ignore-scripts",
        "--no-audit",
        "--no-fund",
        "--registry=https://registry.npmjs.org/",
        "--fetch-timeout=60000",
        "--fetch-retries=1",
    ];
    c.phase(
        InstallPhase::Installing,
        "Resolving exact dependency lock with package scripts disabled",
    );
    let mut args = vec![
        npm.to_string_lossy().into_owned(),
        "install".into(),
        "--package-lock-only".into(),
    ];
    args.extend(common.map(str::to_owned));
    io::run(&node, &args, &project, &env, c, None)?;
    let mut lock: Value = serde_json::from_slice(&fs::read(project.join("package-lock.json"))?)?;
    for (name, pkg) in lock["packages"]
        .as_object_mut()
        .context("npm did not create an exact package lock")?
    {
        if name.is_empty() {
            continue;
        }
        if pkg["inBundle"] == true {
            continue;
        }
        let resolved = str_field(pkg, "resolved")
            .with_context(|| format!("Locked dependency {name}"))?
            .to_owned();
        let url = url::Url::parse(&resolved)?;
        let mut integrity = pkg["integrity"].as_str().unwrap_or("").to_owned();
        if url.scheme() != "https"
            || url.host_str() != Some("registry.npmjs.org")
            || !integrity.starts_with("sha512-")
        {
            // Some upstream shrinkwrap files carry mirror URLs. Replace only after
            // matching the exact package/version integrity against official registry metadata.
            let package_name = pkg["name"].as_str().unwrap_or(
                name.rsplit("node_modules/")
                    .next()
                    .context("Invalid locked package name")?,
            );
            let exact = str_field(pkg, "version")?;
            let metadata = format!(
                "https://registry.npmjs.org/{}/{}",
                package_name.replace('/', "%2f"),
                exact
            );
            let canonical = io::json(&metadata, scratch, c)?;
            let official_integrity = canonical["dist"]["integrity"]
                .as_str()
                .filter(|s| s.starts_with("sha512-"))
                .context("Official dependency lacks SHA512 SRI")?;
            if integrity.starts_with("sha512-") && integrity != official_integrity {
                bail!("Non-registry dependency integrity does not match official package {name}");
            }
            integrity = official_integrity.into();
            pkg["integrity"] = json!(integrity);
            let official = canonical["dist"]["tarball"]
                .as_str()
                .context("Official dependency lacks tarball")?;
            let parsed = url::Url::parse(official)?;
            if parsed.scheme() != "https" || parsed.host_str() != Some("registry.npmjs.org") {
                bail!("Official registry returned an unsafe tarball URL");
            }
            pkg["resolved"] = json!(official);
        }
        artifacts.push(descriptor(
            str_field(pkg, "version")?,
            str_field(pkg, "resolved")?,
            &integrity,
            "package-lock.json",
        ));
    }
    fs::write(
        project.join("package-lock.json"),
        serde_json::to_vec_pretty(&lock)?,
    )?;
    c.phase(
        InstallPhase::Installing,
        "Installing locked packages and verifying registry integrity",
    );
    let mut args = vec![npm.to_string_lossy().into_owned(), "ci".into()];
    args.extend(common.map(str::to_owned));
    io::run(&node, &args, &project, &env, c, None)?;
    let targets: Vec<(&str, &str)> = if package == "web" {
        vec![
            ("html", "vscode-html-language-server"),
            ("css", "vscode-css-language-server"),
            ("json", "vscode-json-language-server"),
        ]
    } else {
        vec![(recipe.languages[0], recipe.executable)]
    };
    let mut configs = Vec::new();
    for (language, exe) in targets {
        let launcher = project
            .join("node_modules/.bin")
            .join(exe)
            .canonicalize()
            .with_context(|| {
                format!(
                    "Package did not install {exe}; a disabled package script may be a prerequisite"
                )
            })?;
        if !launcher.starts_with(&project) {
            bail!("Installed npm launcher escapes its package root");
        }
        let mut args = vec![launcher.to_string_lossy().into_owned()];
        args.extend(recipe.args.iter().map(|s| (*s).into()));
        if package == "typescript" {
            args.extend([
                "--tsserver-path".into(),
                project
                    .join("node_modules/typescript/lib/tsserver.js")
                    .to_string_lossy()
                    .into_owned(),
            ]);
        }
        if package == "vue" {
            args.extend([
                "--tsdk".into(),
                project
                    .join("node_modules/typescript/lib")
                    .to_string_lossy()
                    .into_owned(),
            ]);
        }
        configs.push(env_config(
            language,
            &node,
            args,
            &format!("{bin}:/usr/bin:/bin"),
            &[],
        ));
    }
    Ok((primary, configs))
}
fn jdt(
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
    artifacts: &mut Vec<ToolArtifact>,
) -> Result<(String, Vec<ServerConfig>)> {
    let java = java(payload, scratch, c, artifacts)?;
    let metadata = "https://api.github.com/repos/eclipse-jdtls/eclipse.jdt.ls/tags?per_page=20";
    let tags = io::json(metadata, scratch, c)?;
    let version = tags
        .as_array()
        .context("Invalid JDT tags")?
        .iter()
        .filter_map(|v| v["name"].as_str())
        .filter_map(|v| v.strip_prefix('v'))
        .find(|v| v.split('.').count() == 3 && v.split('.').all(|s| s.parse::<u32>().is_ok()))
        .context("No stable JDT release")?;
    let base = format!("https://download.eclipse.org/jdtls/milestones/{version}/");
    let filename = io::text(&format!("{base}latest.txt"), scratch, c)?
        .trim()
        .to_owned();
    if !filename.starts_with("jdt-language-server-")
        || !filename.ends_with(".tar.gz")
        || filename.contains('/')
        || filename.contains("..")
    {
        bail!("Invalid JDT asset metadata");
    }
    let url = format!("{base}{filename}");
    let sum = io::checksum(&format!("{url}.sha256"), 64, scratch, c)?;
    let a = descriptor(version, &url, &format!("sha256:{sum}"), metadata);
    let server = payload.join("server");
    unpack(&a, &server, scratch, c)?;
    artifacts.push(a);
    let jar = files(&server, &|p| {
        p.file_name().is_some_and(|s| {
            s.to_string_lossy()
                .starts_with("org.eclipse.equinox.launcher_")
                && s.to_string_lossy().ends_with(".jar")
        })
    })?;
    if jar.len() != 1 {
        bail!("JDT archive lacks unique Equinox launcher");
    }
    let config = server.join(if arch()? == "arm64" {
        "config_mac_arm"
    } else {
        "config_mac"
    });
    let config = if config.is_dir() {
        config
    } else {
        server.join("config_mac")
    };
    // Copying the immutable configuration into a session directory is performed by
    // Client::start. JVM reads install/configuration but writes only {workspace}.
    let args = vec![
        "-Declipse.application=org.eclipse.jdt.ls.core.id1".into(),
        "-Dosgi.bundles.defaultStartLevel=4".into(),
        "-Declipse.product=org.eclipse.jdt.ls.core.product".into(),
        "-Dlog.level=WARNING".into(),
        "-Xmx1G".into(),
        "--add-modules=ALL-SYSTEM".into(),
        "--add-opens".into(),
        "java.base/java.util=ALL-UNNAMED".into(),
        "--add-opens".into(),
        "java.base/java.lang=ALL-UNNAMED".into(),
        "-jar".into(),
        jar[0].to_string_lossy().into_owned(),
        "-configuration".into(),
        config.to_string_lossy().into_owned(),
        "-data".into(),
        "{workspace}/data".into(),
    ];
    let home = java.parent().unwrap().parent().unwrap();
    Ok((
        version.into(),
        vec![env_config(
            "java",
            &java,
            args,
            &format!("{}:/usr/bin:/bin", java.parent().unwrap().display()),
            &[("JAVA_HOME", home.to_string_lossy().into_owned())],
        )],
    ))
}
fn kotlin(
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
    artifacts: &mut Vec<ToolArtifact>,
) -> Result<(String, Vec<ServerConfig>)> {
    let release = github("Kotlin/kotlin-lsp", scratch, c)?;
    let body = str_field(&release, "body")?;
    let suffix = if arch()? == "arm64" {
        "-aarch64.sit"
    } else {
        ".sit"
    };
    let url = body
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '(' | ')' | '>' | '<' | '"'))
        .find(|s| {
            s.starts_with("https://download.jetbrains.com/")
                && s.ends_with(suffix)
                && (arch().ok() != Some("x64") || !s.ends_with("-aarch64.sit"))
        })
        .context("Official Kotlin release lacks standalone macOS download")?;
    let sum = io::checksum(&format!("{url}.sha256"), 64, scratch, c)?;
    let version = release["name"]
        .as_str()
        .unwrap_or(str_field(&release, "tag_name")?)
        .trim_start_matches('v');
    let a = descriptor(
        version,
        url,
        &format!("sha256:{sum}"),
        "https://api.github.com/repos/Kotlin/kotlin-lsp/releases/latest",
    );
    unpack(&a, &payload.join("server"), scratch, c)?;
    artifacts.push(a);
    let server = find(&payload.join("server"), "/bin/intellij-server")?;
    archive::executable(&server)?;
    let java = java(payload, scratch, c, artifacts)?;
    let mvn = maven(payload, scratch, c, artifacts)?;
    let home = java.parent().unwrap().parent().unwrap();
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        mvn.parent().unwrap().display(),
        java.parent().unwrap().display()
    );
    Ok((
        version.into(),
        vec![env_config(
            "kotlin",
            &server,
            vec![
                "--stdio".into(),
                "--system-path".into(),
                "{workspace}/system".into(),
                "--data-sharing".into(),
                "none".into(),
            ],
            &path,
            &[
                ("JAVA_HOME", home.to_string_lossy().into_owned()),
                (
                    "MAVEN_OPTS",
                    "-Dmaven.repo.local={workspace}/maven-repository".into(),
                ),
                ("GRADLE_USER_HOME", "{workspace}/gradle-home".into()),
            ],
        )],
    ))
}
fn go_tool(
    package: &str,
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
    artifacts: &mut Vec<ToolArtifact>,
) -> Result<(String, Vec<ServerConfig>)> {
    let source = "https://go.dev/dl/?mode=json";
    let data = io::json(source, scratch, c)?;
    let os_arch = if arch()? == "arm64" { "arm64" } else { "amd64" };
    let release = data
        .as_array()
        .context("Invalid Go releases")?
        .iter()
        .find(|r| r["stable"] == true)
        .context("No stable Go release")?;
    let file = release["files"]
        .as_array()
        .context("Missing Go files")?
        .iter()
        .find(|f| f["os"] == "darwin" && f["arch"] == os_arch && f["kind"] == "archive")
        .context("Missing Go macOS archive")?;
    let a = descriptor(
        str_field(release, "version")?,
        &format!("https://go.dev/dl/{}", str_field(file, "filename")?),
        &format!("sha256:{}", str_field(file, "sha256")?),
        source,
    );
    unpack(&a, &payload.join("runtime"), scratch, c)?;
    artifacts.push(a);
    let go = find(&payload.join("runtime"), "/bin/go")?;
    let goroot = go.parent().unwrap().parent().unwrap();
    let work = scratch.join("go-build");
    fs::create_dir(&work)?;
    let bin = payload.join("bin");
    fs::create_dir(&bin)?;
    let module = match package {
        "go" => "golang.org/x/tools/gopls",
        "sql" => "github.com/sqls-server/sqls",
        _ => "github.com/owenrumney/make-ls",
    };
    let meta_url = format!("https://proxy.golang.org/{module}/@latest");
    let meta = io::json(&meta_url, scratch, c)?;
    let version = str_field(&meta, "Version")?;
    let env = vec![
        ("GOTOOLCHAIN".into(), "local".into()),
        ("GOROOT".into(), goroot.to_string_lossy().into_owned()),
        (
            "GOPATH".into(),
            scratch.join("gopath").to_string_lossy().into_owned(),
        ),
        (
            "GOCACHE".into(),
            scratch.join("gocache").to_string_lossy().into_owned(),
        ),
        ("GOBIN".into(), bin.to_string_lossy().into_owned()),
        ("GOPROXY".into(), "https://proxy.golang.org".into()),
        ("GOSUMDB".into(), "sum.golang.org".into()),
        ("CGO_ENABLED".into(), "0".into()),
        ("GOENV".into(), "off".into()),
    ];
    c.phase(
        InstallPhase::Installing,
        "Verifying official Go module through checksum database",
    );
    let download = io::run(
        &go,
        &[
            "mod".into(),
            "download".into(),
            "-json".into(),
            format!("{module}@{version}"),
        ],
        &work,
        &env,
        c,
        None,
    )?;
    let record: Value = serde_json::from_str(&download)?;
    artifacts.push(descriptor(
        version,
        &format!("https://proxy.golang.org/{module}/@v/{version}.zip"),
        str_field(&record, "Sum")?,
        &meta_url,
    ));
    let target = if package == "go" || package == "sql" {
        format!("{module}@{version}")
    } else {
        format!("{module}/cmd/make-ls@{version}")
    };
    c.phase(
        InstallPhase::Installing,
        "Building verified language tool in app-owned scratch",
    );
    if package == "sql" {
        // Upstream unconditionally imports the Oracle CGO driver. Build a documented
        // pure-Go variant from its checksum-database-verified module, omitting only
        // Oracle's self-registering driver. No project sources or build scripts run.
        let source = PathBuf::from(str_field(&record, "Dir")?);
        let build = work.join("sqls-source");
        fn copy_tree(from: &Path, to: &Path, c: &InstallControl) -> Result<()> {
            fs::create_dir(to)?;
            for e in fs::read_dir(from)? {
                c.check()?;
                let e = e?;
                let target = to.join(e.file_name());
                let kind = e.file_type()?;
                if kind.is_dir() {
                    copy_tree(&e.path(), &target, c)?;
                } else if kind.is_file() {
                    fs::copy(e.path(), target)?;
                } else {
                    bail!("Unexpected link in verified Go module");
                }
            }
            Ok(())
        }
        copy_tree(&source, &build, c)?;
        let oracle = build.join("internal/database/oracle.go");
        let text = fs::read_to_string(&oracle)?;
        if !text.contains("github.com/godror/godror") || !text.contains("RegisterOpen(\"oracle\"") {
            bail!("SQLs Oracle adapter source changed; catalog update required");
        }
        fs::remove_file(oracle)?;
        fs::write(
            payload.join("NOTICE-FORKARAK-SQL.txt"),
            "SQLs is built from checksum-database-verified upstream source with CGO disabled. The Oracle driver registration is omitted. SQLite CGO bindings are unavailable. MySQL/PostgreSQL and static SQL language features are retained. No SQL execution commands are exposed by Forkarak.\n",
        )?;
        io::run(
            &go,
            &[
                "build".into(),
                "-mod=readonly".into(),
                "-o".into(),
                bin.join("sqls").to_string_lossy().into_owned(),
                ".".into(),
            ],
            &build,
            &env,
            c,
            None,
        )?;
    } else {
        io::run(&go, &["install".into(), target], &work, &env, c, None)?;
    }
    let exe = bin.join(match package {
        "go" => "gopls",
        "sql" => "sqls",
        _ => "make-ls",
    });
    let config = env_config(
        match package {
            "go" => "go",
            "sql" => "sql",
            _ => "makefile",
        },
        &exe,
        vec![],
        &format!("{}:/usr/bin:/bin", go.parent().unwrap().display()),
        &[
            ("GOROOT", goroot.to_string_lossy().into_owned()),
            ("GOTOOLCHAIN", "local".into()),
            ("GOPATH", "{workspace}/gopath".into()),
            ("GOCACHE", "{workspace}/go-cache".into()),
            ("GOENV", "off".into()),
        ],
    );
    Ok((version.into(), vec![config]))
}
fn xml(
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
    artifacts: &mut Vec<ToolArtifact>,
) -> Result<(String, Vec<ServerConfig>)> {
    // Red Hat publishes the maintained LemMinX distribution in the official XML
    // extension's platform-independent VSIX (ZIP). Use Open VSX publisher metadata.
    let metadata = "https://open-vsx.org/api/redhat/vscode-xml/latest";
    let release = io::json(metadata, scratch, c)?;
    let version = str_field(&release, "version")?;
    let url = release["files"]["download"]
        .as_str()
        .context("XML extension missing archive")?;
    let checksum_url = release["files"]["sha256"]
        .as_str()
        .context("XML extension missing official SHA256")?;
    let sum = io::checksum(checksum_url, 64, scratch, c)?;
    let a = descriptor(version, url, &format!("sha256:{sum}"), metadata);
    unpack(&a, &payload.join("server"), scratch, c)?;
    artifacts.push(a);
    let jar = find(&payload.join("server"), "/org.eclipse.lemminx-uber.jar")?;
    let java = java(payload, scratch, c, artifacts)?;
    let config = env_config(
        "xml",
        &java,
        vec!["-jar".into(), jar.to_string_lossy().into_owned()],
        &format!("{}:/usr/bin:/bin", java.parent().unwrap().display()),
        &[],
    );
    Ok((version.into(), vec![config]))
}
fn rust_toolchain(
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
    artifacts: &mut Vec<ToolArtifact>,
) -> Result<PathBuf> {
    let metadata = "https://static.rust-lang.org/dist/channel-rust-stable.toml";
    let text = io::text(metadata, scratch, c)?;
    let manifest: toml::Value = toml::from_str(&text)?;
    let triple = if arch()? == "arm64" {
        "aarch64-apple-darwin"
    } else {
        "x86_64-apple-darwin"
    };
    let destination = payload.join("toolchain");
    fs::create_dir(&destination)?;
    fn copy(from: &Path, to: &Path, root: &Path, depth: usize, c: &InstallControl) -> Result<()> {
        c.check()?;
        if depth > 64 {
            bail!("Rust component link cycle or excessive depth");
        }
        let real = from.canonicalize()?;
        if !real.starts_with(root) {
            bail!("Rust component escapes verified archive");
        }
        if real.is_dir() {
            fs::create_dir_all(to)?;
            for e in fs::read_dir(&real)? {
                let e = e?;
                copy(&e.path(), &to.join(e.file_name()), root, depth + 1, c)?;
            }
        } else {
            fs::copy(real, to)?;
        }
        Ok(())
    }
    for name in ["rustc", "cargo", "rust-std", "rustfmt-preview"] {
        let package = &manifest["pkg"][name];
        let target = &package["target"][triple];
        if target["available"].as_bool() != Some(true) {
            bail!("Official Rust component unavailable for this Mac");
        }
        let url = target["url"]
            .as_str()
            .context("Rust component URL missing")?;
        let hash = target["hash"]
            .as_str()
            .context("Rust component SHA256 missing")?;
        let version = package["version"]
            .as_str()
            .context("Rust version missing")?
            .split_whitespace()
            .next()
            .context("Empty Rust version")?;
        let a = descriptor(version, url, &format!("sha256:{hash}"), metadata);
        let extracted = scratch.join(format!("rust-{name}"));
        unpack(&a, &extracted, scratch, c)?;
        let entries: Vec<_> = fs::read_dir(&extracted)?.collect::<std::io::Result<Vec<_>>>()?;
        if entries.len() != 1 || !entries[0].file_type()?.is_dir() {
            bail!("Invalid Rust component archive root");
        }
        let root = entries[0].path();
        let component = root.join(if name == "rust-std" {
            format!("rust-std-{triple}")
        } else {
            name.into()
        });
        if !component.is_dir() {
            bail!("Rust archive lacks expected component directory");
        }
        for sub in ["bin", "lib", "libexec", "share", "etc"] {
            let from = component.join(sub);
            if from.exists() {
                copy(
                    &from,
                    &destination.join(sub),
                    &extracted.canonicalize()?,
                    0,
                    c,
                )?;
            }
        }
        artifacts.push(a);
    }
    Ok(destination)
}

fn groovy(
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
    artifacts: &mut Vec<ToolArtifact>,
) -> Result<(String, Vec<ServerConfig>)> {
    // Reviewed source/dependency pins are embedded in the app. Never run the
    // downloaded project's Gradle wrapper, plugins, build scripts or processors.
    let pins: Value = serde_json::from_str(include_str!("../../resources/managed-pins.json"))?;
    let pinned: Vec<ToolArtifact> = serde_json::from_value(pins["groovy"].clone())?;
    let source = payload.join("source");
    unpack(&pinned[0], &source, scratch, c)?;
    let lib = payload.join("lib");
    fs::create_dir(&lib)?;
    let mut jars = Vec::new();
    for a in pinned.iter().skip(1) {
        let file = io::artifact(a, scratch, c)?;
        let name = url::Url::parse(&a.source_url)?
            .path_segments()
            .and_then(|mut s| s.next_back())
            .context("Dependency JAR name missing")?
            .to_owned();
        let target = lib.join(name);
        fs::copy(file, &target)?;
        jars.push(target);
    }
    let java = java(payload, scratch, c, artifacts)?;
    let javac = java.parent().unwrap().join("javac");
    let classes = payload.join("classes");
    fs::create_dir(&classes)?;
    let classpath = jars
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join(":");
    let mut args = vec![
        "-proc:none".into(),
        "--release".into(),
        "8".into(),
        "-classpath".into(),
        classpath.clone(),
        "-d".into(),
        classes.to_string_lossy().into_owned(),
    ];
    let sources = files(&source, &|p| {
        p.extension().is_some_and(|e| e == "java")
            && p.to_string_lossy().contains("/src/main/java/")
    })?;
    if sources.is_empty() {
        bail!("Reviewed Groovy source archive contains no main Java sources");
    }
    args.extend(sources.iter().map(|p| p.to_string_lossy().into_owned()));
    c.phase(InstallPhase::Installing,"Compiling pinned Groovy server using managed javac; processors and project builds disabled");
    io::run(&javac, &args, scratch, &[], c, None)?;
    artifacts.extend(pinned.iter().cloned());
    let runtime_classpath = format!("{}:{classpath}", classes.display());
    Ok((
        pinned[0].version.clone(),
        vec![env_config(
            "groovy",
            &java,
            vec![
                "-cp".into(),
                runtime_classpath,
                "net.prominic.groovyls.GroovyLanguageServer".into(),
            ],
            &format!("{}:/usr/bin:/bin", java.parent().unwrap().display()),
            &[],
        )],
    ))
}

pub(super) fn install(
    package: &str,
    payload: &Path,
    scratch: &Path,
    c: &InstallControl,
) -> Result<InstalledTool> {
    arch()?;
    let recipe = RECIPES
        .iter()
        .find(|r| r.id == package)
        .context("Unknown package")?;
    let mut artifacts = Vec::new();
    let (version, servers) = if !recipe.npm.is_empty() {
        npm(package, payload, scratch, c, &mut artifacts)?
    } else {
        match package {
            "groovy" => groovy(payload, scratch, c, &mut artifacts)?,
            "java" => jdt(payload, scratch, c, &mut artifacts)?,
            "kotlin" => kotlin(payload, scratch, c, &mut artifacts)?,
            "go" | "makefile" | "sql" => go_tool(package, payload, scratch, c, &mut artifacts)?,
            "xml" => xml(payload, scratch, c, &mut artifacts)?,
            "toml" => {
                let (name, sum) = if arch()? == "arm64" {
                    (
                        "taplo-darwin-aarch64.gz",
                        "713734314c3e71894b9e77513c5349835eefbd52908445a0d73b0c7dc469347d",
                    )
                } else {
                    (
                        "taplo-darwin-x86_64.gz",
                        "898122cde3a0b1cd1cbc2d52d3624f23338218c91b5ddb71518236a4c2c10ef2",
                    )
                };
                let a = descriptor(
                    "0.10.0",
                    &format!("https://github.com/tamasfe/taplo/releases/download/0.10.0/{name}"),
                    &format!("sha256:{sum}"),
                    "https://api.github.com/repos/tamasfe/taplo/releases/tags/0.10.0",
                );
                let file = io::artifact(&a, scratch, c)?;
                let binary = payload.join("taplo");
                archive::gunzip(&file, &binary, c)?;
                artifacts.push(a);
                (
                    "0.10.0".into(),
                    vec![env_config(
                        "toml",
                        &binary,
                        vec!["lsp".into(), "stdio".into()],
                        "/usr/bin:/bin",
                        &[],
                    )],
                )
            }
            "rust" | "markdown" => {
                let repo = if package == "rust" {
                    "rust-lang/rust-analyzer"
                } else {
                    "artempyanykh/marksman"
                };
                let release = github(repo, scratch, c)?;
                let name = if package == "rust" {
                    format!(
                        "rust-analyzer-{}-apple-darwin.gz",
                        if arch()? == "arm64" {
                            "aarch64"
                        } else {
                            "x86_64"
                        }
                    )
                } else {
                    "marksman-macos".into()
                };
                let a = github_asset(&release, &name, scratch, c)?;
                let file = io::artifact(&a, scratch, c)?;
                let binary = payload.join(recipe.executable);
                if package == "rust" {
                    archive::gunzip(&file, &binary, c)?;
                } else {
                    fs::copy(file, &binary)?;
                    archive::executable(&binary)?;
                }
                let version = a.version.clone();
                artifacts.push(a);
                let path = if package == "rust" {
                    format!(
                        "{}:/usr/bin:/bin",
                        rust_toolchain(payload, scratch, c, &mut artifacts)?
                            .join("bin")
                            .display()
                    )
                } else {
                    "/usr/bin:/bin".into()
                };
                (
                    version,
                    vec![env_config(
                        recipe.languages[0],
                        &binary,
                        recipe.args.iter().map(|a| (*a).into()).collect(),
                        &path,
                        &[
                            ("CARGO_HOME", "{workspace}/cargo-home".into()),
                            ("RUSTUP_HOME", "{workspace}/rustup-home".into()),
                        ],
                    )],
                )
            }
            _ => bail!("No verified installation recipe for {package}"),
        }
    };
    let mut notices: Vec<String> = files(payload, &|p| {
        p.file_name().is_some_and(|s| {
            let n = s.to_string_lossy().to_ascii_lowercase();
            n.starts_with("license") || n.starts_with("notice") || n == "legal" || n == "release"
        })
    })?
    .iter()
    .map(|p| p.to_string_lossy().into_owned())
    .collect();
    notices.insert(
        0,
        format!(
            "{}: {}; official source {}",
            recipe.name, recipe.license, recipe.source
        ),
    );
    Ok(InstalledTool {
        package: package.into(),
        version,
        distribution: ToolDistribution::FirstUseDownload,
        artifacts,
        servers,
        formatters: vec![],
        notices,
    })
}
