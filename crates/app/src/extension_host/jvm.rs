//! Build discovery shared by the built-in Spring extension and editor test actions.
mod gradle;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Component, Path, PathBuf},
};

/// Resolve existing symlinks, including any prefix of a not-yet-created report path.
pub(crate) fn confined(root: &Path, path: &Path) -> Result<PathBuf> {
    let mut result = root.to_owned();
    for component in path.strip_prefix(root)?.components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("워크트리 밖의 경로입니다");
        };
        result.push(name);
        match fs::symlink_metadata(&result) {
            Ok(metadata) if metadata.is_symlink() => {
                result = result.canonicalize()?;
                ensure!(result.starts_with(root), "워크트리 밖의 경로입니다");
            }
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    Ok(result)
}

pub(crate) fn file(root: &Path, path: &Path) -> Result<Option<PathBuf>> {
    let path = confined(root, path)?;
    Ok(path.is_file().then_some(path))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmBuild {
    Maven,
    Gradle,
}

pub(super) fn builds_at(root: &Path, dir: &Path) -> Result<Vec<JvmBuild>> {
    let mut builds = Vec::new();
    if file(root, &dir.join("pom.xml"))?.is_some() {
        builds.push(JvmBuild::Maven);
    }
    if file(root, &dir.join("build.gradle"))?.is_some()
        || file(root, &dir.join("build.gradle.kts"))?.is_some()
    {
        builds.push(JvmBuild::Gradle);
    }
    Ok(builds)
}

pub(crate) struct Project {
    pub kind: JvmBuild,
    pub module: PathBuf,
    pub build_root: PathBuf,
}

impl Project {
    pub fn closest(root: &Path, source: &Path) -> Result<Option<Self>> {
        for module in source
            .ancestors()
            .skip(1)
            .take_while(|p| p.starts_with(root))
        {
            if let Some(kind) = builds_at(root, module)?.first().copied() {
                let build_root = match kind {
                    JvmBuild::Gradle => gradle_root(root, module)?,
                    JvmBuild::Maven => reactor_root(root, module)?,
                };
                return Ok(Some(Self {
                    kind,
                    module: module.to_owned(),
                    build_root,
                }));
            }
        }
        Ok(None)
    }

    pub fn gradle_test_task(&self, root: &Path) -> Result<String> {
        gradle::task(root, &self.build_root, &self.module)
    }

    pub fn program(&self, root: &Path, settings: &Value) -> Result<String> {
        let (setting, wrapper, fallback) = match self.kind {
            JvmBuild::Gradle => ("spring.gradle", "gradlew", "gradle"),
            JvmBuild::Maven => ("spring.maven", "mvnw", "mvn"),
        };
        if let Some(configured) = settings[setting].as_str().filter(|s| !s.is_empty()) {
            let path = Path::new(configured);
            return if path.is_relative() && configured.contains('/') {
                Ok(confined(root, &root.join(path))?
                    .to_string_lossy()
                    .into_owned())
            } else {
                Ok(configured.to_owned())
            };
        }
        for dir in self
            .module
            .ancestors()
            .take_while(|p| p.starts_with(&self.build_root))
        {
            if let Some(path) = file(root, &dir.join(wrapper))? {
                return Ok(path.to_string_lossy().into_owned());
            }
        }
        Ok(fallback.to_owned())
    }
}

fn gradle_root(root: &Path, module: &Path) -> Result<PathBuf> {
    for dir in module.ancestors().take_while(|dir| dir.starts_with(root)) {
        for settings in ["settings.gradle", "settings.gradle.kts"] {
            if file(root, &dir.join(settings))?.is_some() {
                return Ok(dir.to_owned());
            }
        }
    }
    // Without settings declaring subprojects, the build file is a standalone project.
    Ok(module.to_owned())
}

fn reactor_root(root: &Path, module: &Path) -> Result<PathBuf> {
    let mut reactor = module;
    for dir in module
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(root))
    {
        let Some(pom) = file(root, &dir.join("pom.xml"))? else {
            continue;
        };
        let source = fs::read_to_string(pom)?;
        let doc = roxmltree::Document::parse(&source).context("pom.xml을 읽을 수 없습니다")?;
        // A parent POM or an unrelated project with its own wrapper need not aggregate this module.
        let includes = doc
            .root_element()
            .children()
            .filter(|node| node.has_tag_name("modules") || node.has_tag_name("subprojects"))
            .flat_map(|group| group.children())
            .filter(|node| node.has_tag_name("module") || node.has_tag_name("subproject"))
            .filter_map(|node| node.text())
            .any(|entry| {
                let path = dir.join(entry.trim());
                path == reactor || path == reactor.join("pom.xml")
            });
        if includes {
            reactor = dir;
        }
    }
    Ok(reactor.to_owned())
}

pub(crate) fn environment(root: &Path, settings: &Value) -> Result<BTreeMap<String, String>> {
    let mut env: BTreeMap<String, String> = settings
        .get("spring.env")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    if let Some(jdk) = settings["spring.jdk"].as_str().filter(|s| !s.is_empty()) {
        let path = Path::new(jdk);
        let home = if path.is_absolute() {
            path.canonicalize()
        } else {
            confined(root, &root.join(path))?.canonicalize()
        }
        .context("JDK 홈을 찾을 수 없습니다")?;
        ensure!(
            home.join("bin/java").is_file(),
            "JDK 홈에 bin/java가 없습니다"
        );
        env.insert("JAVA_HOME".into(), home.display().to_string());
        let path = env
            .get("PATH")
            .cloned()
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
        env.insert("PATH".into(), format!("{}/bin:{path}", home.display()));
    }
    Ok(env)
}
