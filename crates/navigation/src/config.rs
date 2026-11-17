use crate::ServerConfig;
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// All waits happen on the caller's background thread. Construction never starts a server.
#[derive(Clone, Debug)]
pub struct NavigatorOptions {
    pub initialize_timeout: Duration,
    pub request_timeout: Duration,
    pub shutdown_timeout: Duration,
    /// None disables user configuration, useful for isolated callers and tests.
    pub config_file: Option<PathBuf>,
    /// Kotlin servers can run project import tools. Requires explicit opt-in.
    pub allow_project_imports: bool,
}

impl Default for NavigatorOptions {
    fn default() -> Self {
        Self {
            initialize_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(2),
            config_file: env::var_os("HOME")
                .map(|p| PathBuf::from(p).join(".config/pokarak/language-servers.json")),
            allow_project_imports: false,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    servers: Vec<ServerConfig>,
    #[serde(default, rename = "allowProjectImports")]
    allow_project_imports: bool,
}

pub(crate) fn merge_user_config(
    configs: &mut Vec<ServerConfig>,
    options: &mut NavigatorOptions,
) -> Result<()> {
    if let Some(path) = &options.config_file {
        let text = match fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Cannot read language-server config {}", path.display())
                });
            }
        };
        if let Some(text) = text {
            let user: ConfigFile = serde_json::from_str(&text)
                .with_context(|| format!("Invalid language-server config {}", path.display()))?;
            options.allow_project_imports |= user.allow_project_imports;
            for mut config in user.servers {
                config.language = normalize_language(&config.language).into();
                configs.retain(|old| normalize_language(&old.language) != config.language);
                configs.push(config);
            }
        }
    }
    for config in configs {
        config.language = normalize_language(&config.language).into();
        if !matches!(
            config.language.as_str(),
            "typescript" | "typescriptreact" | "javascript" | "javascriptreact" | "java" | "kotlin"
        ) {
            bail!("Unsupported language-server language {:?}", config.language);
        }
        if config.program.trim().is_empty() {
            bail!("Empty language-server program for {}", config.language);
        }
    }
    Ok(())
}

pub(crate) fn normalize_language(language: &str) -> &str {
    match language {
        "ts" | "js/ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" => "javascript",
        "jsx" => "javascriptreact",
        "kt" | "kts" => "kotlin",
        other => other,
    }
}

pub(crate) fn language(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("typescriptreact"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("javascriptreact"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        _ => None,
    }
}

pub(crate) fn family(language: &str) -> &str {
    match language {
        "typescriptreact" | "javascript" | "javascriptreact" => "typescript",
        other => other,
    }
}

/// Discover an executable on the inherited PATH. Never searches project node_modules,
/// invokes a shell, runs npx, installs software, or starts the discovered program.
pub fn discover_program(program: &str) -> Option<PathBuf> {
    find_on_path(program, &env::var_os("PATH").unwrap_or_default())
}

fn find_on_path(program: &str, search_path: &OsStr) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() {
        return executable(path).then(|| path.to_path_buf());
    }
    if path.components().count() != 1 {
        return None;
    }
    env::split_paths(search_path)
        // Relative PATH entries could execute code in the inspected repository.
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|path| executable(path))
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

// The official Homebrew link, native standalone executable, and legacy wrapper.
const KOTLIN_LAUNCHERS: [&str; 3] = ["kotlin-lsp", "intellij-server", "kotlin-lsp.sh"];

fn kotlin_directories(
    home: Option<&Path>,
    distribution: Option<&Path>,
    brew_prefixes: &[PathBuf],
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dist) = distribution.filter(|p| p.is_absolute()) {
        dirs.extend([dist.join("bin"), dist.to_path_buf()]);
    }
    if let Some(home) = home.filter(|p| p.is_absolute()) {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".local/share/kotlin-lsp/bin"));
        dirs.push(home.join(".local/share/kotlin-lsp"));
    }
    for prefix in brew_prefixes.iter().filter(|p| p.is_absolute()) {
        dirs.push(prefix.join("bin"));
        dirs.push(prefix.join("opt/kotlin-lsp/bin"));
        dirs.push(prefix.join("opt/kotlin-lsp/libexec/bin"));
    }
    dirs
}

fn find_kotlin(names: &[&str], search_path: &OsStr, extra: &[PathBuf]) -> Option<PathBuf> {
    names
        .iter()
        .find_map(|name| find_on_path(name, search_path))
        .or_else(|| {
            extra
                .iter()
                .filter(|dir| dir.is_absolute())
                .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
                .find(|path| executable(path))
        })
}

fn installed_kotlin(names: &[&str]) -> Option<PathBuf> {
    let mut brew: Vec<PathBuf> = env::var_os("HOMEBREW_PREFIX")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if cfg!(target_os = "macos") {
        let defaults = if cfg!(target_arch = "aarch64") {
            ["/opt/homebrew", "/usr/local"]
        } else {
            ["/usr/local", "/opt/homebrew"]
        };
        brew.extend(defaults.map(PathBuf::from));
    }
    let home = env::var_os("HOME").map(PathBuf::from);
    let distribution = env::var_os("KOTLIN_LSP_DIR").map(PathBuf::from);
    find_kotlin(
        names,
        &env::var_os("PATH").unwrap_or_default(),
        &kotlin_directories(home.as_deref(), distribution.as_deref(), &brew),
    )
}

// Preserve a configured program/argv pair. For a managed /usr/bin/env wrapper,
// verify its absolute native payload before any project import discovery.
pub(crate) fn kotlin_program(config: &ServerConfig) -> Result<PathBuf> {
    let found = if KOTLIN_LAUNCHERS.contains(&config.program.as_str()) {
        installed_kotlin(&[config.program.as_str()])
    } else {
        discover_program(&config.program)
    };
    let unavailable = |path: &Path| {
        anyhow!(
            "Language server unavailable for kotlin: {} 실행 파일을 찾을 수 없거나 실행 권한이 없습니다.\nJetBrains Kotlin LSP 설치 명령은 brew install JetBrains/utils/kotlin-lsp 입니다. 직접 설치했다면 ~/.config/pokarak/language-servers.json의 kotlin program에 bin/intellij-server의 절대 경로를 지정하고 분석을 다시 활성화하세요.",
            path.display()
        )
    };
    let program = found.ok_or_else(|| unavailable(Path::new(&config.program)))?;
    if let Some(missing) = missing_env_payload(config) {
        return Err(unavailable(missing));
    }
    Ok(program)
}

pub fn default_configs() -> Vec<ServerConfig> {
    // A family entry also covers JS/JSX/TSX; exact language overrides take priority.
    let mut configs = [
        ("typescript", "typescript-language-server", vec!["--stdio"]),
        (
            "java",
            "jdtls",
            vec![
                "-data",
                "{workspace}",
                "-configuration",
                "{workspace}/configuration",
            ],
        ),
        (
            "kotlin",
            "kotlin-lsp",
            vec![
                "--stdio",
                "--system-path",
                "{workspace}/system",
                "--data-sharing",
                "none",
            ],
        ),
    ]
    .into_iter()
    .map(|(language, program, args)| ServerConfig {
        language: language.into(),
        program: (if language == "kotlin" {
            installed_kotlin(&KOTLIN_LAUNCHERS)
        } else {
            discover_program(program)
        })
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.into()),
        args: args.into_iter().map(str::to_owned).collect(),
    })
    .collect::<Vec<_>>();
    if let Some(home) = env::var_os("HOME") {
        apply_managed_configs(
            &mut configs,
            &PathBuf::from(home).join(".local/share/pokarak/tools"),
        );
    }
    configs
}

fn apply_managed_configs(configs: &mut Vec<ServerConfig>, tools_directory: &Path) {
    if let Ok(installed) = read_manifest(tools_directory) {
        for config in installed {
            if missing_managed_program(&config).is_none() {
                configs.retain(|entry| entry.language != config.language);
                configs.push(config);
            }
        }
    }
}

/// Read setup's manifest only. No downloads, scripts, or server startup.
pub fn installed_configs(tools_directory: &Path) -> Result<Vec<ServerConfig>> {
    let configs = read_manifest(tools_directory)?;
    for config in &configs {
        if let Some(missing) = missing_managed_program(config) {
            bail!(
                "Installed executable is unavailable: {}. Rerun scripts/setup-language-servers.sh",
                missing.display()
            );
        }
    }
    Ok(configs)
}

fn missing_managed_program(config: &ServerConfig) -> Option<&Path> {
    let path = Path::new(&config.program);
    if !path.is_absolute() || !executable(path) {
        return Some(path);
    }
    missing_env_payload(config)
}

fn missing_env_payload(config: &ServerConfig) -> Option<&Path> {
    env_kotlin_payload(config).filter(|p| p.is_absolute() && !executable(p))
}

fn read_manifest(tools_directory: &Path) -> Result<Vec<ServerConfig>> {
    #[derive(Deserialize)]
    struct Manifest {
        #[serde(rename = "schemaVersion")]
        schema_version: u32,
        servers: Vec<ServerConfig>,
    }
    let path = tools_directory.join("installed.json");
    let manifest: Manifest = serde_json::from_str(&fs::read_to_string(&path)?)
        .with_context(|| format!("Invalid tools manifest {}", path.display()))?;
    if manifest.schema_version != 1 {
        bail!("Unsupported tools manifest version");
    }
    let mut configs = manifest.servers;
    let mut options = NavigatorOptions {
        config_file: None,
        ..NavigatorOptions::default()
    };
    merge_user_config(&mut configs, &mut options)?;
    Ok(configs)
}

fn kotlin_launcher(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| KOTLIN_LAUNCHERS.contains(&name))
}

fn env_kotlin_payload(config: &ServerConfig) -> Option<&Path> {
    if config.program != "/usr/bin/env" {
        return None;
    }
    let payload = Path::new(config.args.iter().find(|arg| !arg.contains('='))?);
    kotlin_launcher(payload).then_some(payload)
}

pub(crate) fn is_jetbrains(config: &ServerConfig) -> bool {
    kotlin_launcher(Path::new(&config.program)) || env_kotlin_payload(config).is_some()
}

pub(crate) fn is_jdt(config: &ServerConfig) -> bool {
    Path::new(&config.program)
        .file_name()
        .and_then(|name| name.to_str())
        == Some("jdtls")
        || config
            .args
            .iter()
            .any(|arg| arg == "-Declipse.application=org.eclipse.jdt.ls.core.id1")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn put_launcher(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink("/usr/bin/true", path).unwrap();
    }

    #[test]
    fn finds_native_distribution_and_homebrew_launchers_without_a_login_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let dist = tmp.path().join("standalone distribution");
        let brew = tmp.path().join("brew");
        let dirs = kotlin_directories(Some(&home), Some(&dist), std::slice::from_ref(&brew));
        // Neither the shell nor real, machine-wide install locations are consulted.
        let gui_path = env::join_paths([tmp.path().join("system/bin")]).unwrap();
        for candidate in [
            brew.join("opt/kotlin-lsp/libexec/bin/intellij-server"),
            brew.join("bin/kotlin-lsp"),
            home.join(".local/bin/kotlin-lsp"),
            dist.join("bin/intellij-server"),
        ] {
            put_launcher(&candidate);
            assert_eq!(
                find_kotlin(&KOTLIN_LAUNCHERS, &gui_path, &dirs),
                Some(candidate)
            );
        }
        let path_launcher = tmp.path().join("system/bin/kotlin-lsp.sh");
        put_launcher(&path_launcher);
        assert_eq!(
            find_kotlin(&KOTLIN_LAUNCHERS, &gui_path, &dirs),
            Some(path_launcher)
        );
        assert!(
            kotlin_directories(
                Some(Path::new(".")),
                Some(Path::new("bin")),
                &[PathBuf::from(".")]
            )
            .is_empty()
        );

        let unopened = tmp.path().join("non-executable");
        fs::write(&unopened, b"not executable").unwrap();
        fs::set_permissions(&unopened, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(find_on_path(unopened.to_str().unwrap(), &gui_path), None);
    }

    #[test]
    fn a_stale_managed_entry_preserves_working_servers_and_their_argv() {
        let tmp = tempfile::tempdir().unwrap();
        let launcher = tmp.path().join("native distribution/bin/intellij-server");
        put_launcher(&launcher);
        let missing = tmp.path().join("stale/bin/node");
        fs::write(tmp.path().join("installed.json"), serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "servers": [
                {"language":"typescript","program":missing,"args":[]},
                {"language":"kotlin","program":"/usr/bin/env","args":["PATH=/bin",launcher,"--stdio"]}
            ]
        })).unwrap()).unwrap();
        let mut configs = vec![ServerConfig {
            language: "typescript".into(),
            program: "typescript-language-server".into(),
            args: vec!["--stdio".into()],
        }];
        apply_managed_configs(&mut configs, tmp.path());
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].program, "typescript-language-server");
        assert_eq!(configs[1].args[1], launcher.to_str().unwrap());
        assert!(is_jetbrains(&configs[1]));
        fs::remove_file(&launcher).unwrap();
        assert_eq!(
            missing_managed_program(&configs[1]),
            Some(launcher.as_path())
        );
        assert!(
            kotlin_program(&configs[1])
                .unwrap_err()
                .to_string()
                .contains(launcher.to_str().unwrap())
        );
        configs.truncate(1);
        apply_managed_configs(&mut configs, tmp.path());
        assert_eq!(configs.len(), 1);
    }
}
