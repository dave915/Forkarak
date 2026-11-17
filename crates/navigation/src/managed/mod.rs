//! Explicit, versioned app-owned tool installation. Construction/status never install or execute.
mod archive;
mod io;
mod recipes;

use crate::{CancellationHandle, FormatterConfig, ServerConfig, languages::RECIPES};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolDistribution {
    FirstUseDownload,
    Bundled,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolPackage {
    pub id: String,
    pub name: String,
    pub languages: Vec<String>,
    pub source_url: String,
    pub license: String,
    pub distribution: ToolDistribution,
    pub prerequisites: Vec<String>,
    pub limitations: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolArtifact {
    pub version: String,
    pub source_url: String,
    pub checksum: String,
    #[serde(default)]
    pub metadata_url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstalledTool {
    pub package: String,
    pub version: String,
    pub distribution: ToolDistribution,
    pub artifacts: Vec<ToolArtifact>,
    pub servers: Vec<ServerConfig>,
    pub formatters: Vec<FormatterConfig>,
    pub notices: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolStatus {
    pub package: String,
    pub installed: Option<InstalledTool>,
    pub problem: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallPhase {
    Queued,
    Resolving,
    Downloading,
    Verifying,
    Extracting,
    Installing,
    Activating,
    Complete,
    Cancelled,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstallProgress {
    pub package: String,
    pub phase: InstallPhase,
    pub message: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}
struct Operation {
    progress: Mutex<InstallProgress>,
    result: Mutex<Option<std::result::Result<InstalledTool, String>>>,
    done: Condvar,
    cancellation: CancellationHandle,
    // Serializes cancellation against the final manifest rename. Once Complete,
    // cancel is a no-op and never falsely reports a committed install as cancelled.
    activation: Mutex<()>,
    committed: AtomicBool,
}
#[derive(Clone)]
pub struct InstallControl {
    inner: Arc<Operation>,
}
impl InstallControl {
    fn new(package: &str) -> Self {
        Self {
            inner: Arc::new(Operation {
                progress: Mutex::new(InstallProgress {
                    package: package.into(),
                    phase: InstallPhase::Queued,
                    message: "Queued".into(),
                    downloaded_bytes: 0,
                    total_bytes: None,
                    error: None,
                }),
                result: Mutex::new(None),
                done: Condvar::new(),
                cancellation: CancellationHandle::default(),
                activation: Mutex::new(()),
                committed: AtomicBool::new(false),
            }),
        }
    }
    pub fn cancel(&self) -> Result<()> {
        let _guard = self
            .inner
            .activation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.inner.committed.load(Ordering::Acquire)
            || self
                .inner
                .result
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
        {
            return Ok(());
        }
        self.inner.cancellation.cancel()
    }
    pub fn snapshot(&self) -> InstallProgress {
        self.inner
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    /// Blocking; use only on a background thread, never on the GPUI thread.
    pub fn wait(&self) -> Result<InstalledTool> {
        let mut result = self.inner.result.lock().unwrap_or_else(|e| e.into_inner());
        while result.is_none() {
            result = self
                .inner
                .done
                .wait(result)
                .unwrap_or_else(|e| e.into_inner());
        }
        result.as_ref().unwrap().clone().map_err(anyhow::Error::msg)
    }
    fn check(&self) -> Result<()> {
        if self.inner.cancellation.is_cancelled() {
            bail!("Tool installation cancelled");
        }
        Ok(())
    }
    fn phase(&self, phase: InstallPhase, message: impl Into<String>) {
        let mut p = self
            .inner
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        p.phase = phase;
        p.message = message.into();
        p.downloaded_bytes = 0;
        p.total_bytes = None;
    }
    fn bytes(&self, bytes: u64, total: Option<u64>) {
        let mut p = self
            .inner
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        p.downloaded_bytes = bytes;
        p.total_bytes = total.filter(|n| *n >= bytes);
    }
    fn finish(&self, result: Result<InstalledTool>) {
        let mut p = self
            .inner
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match &result {
            Ok(_) => {
                p.phase = InstallPhase::Complete;
                p.message = "Installed; analysis still requires explicit activation".into();
            }
            Err(e) => {
                p.phase = if self.inner.cancellation.is_cancelled() {
                    InstallPhase::Cancelled
                } else {
                    InstallPhase::Failed
                };
                p.message = format!("{e:#}");
                p.error = Some(p.message.clone());
            }
        }
        drop(p);
        *self.inner.result.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(result.map_err(|e| format!("{e:#}")));
        self.inner.done.notify_all();
    }
}

#[derive(Clone)]
pub struct ManagedTools {
    root: PathBuf,
    resources: Option<PathBuf>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u32,
    packages: BTreeMap<String, InstalledTool>,
}
impl ManagedTools {
    pub fn new(root: PathBuf, bundled_resources: Option<PathBuf>) -> Result<Self> {
        validate_absolute(&root)?;
        if let Some(path) = &bundled_resources {
            validate_absolute(path)?;
        }
        Ok(Self {
            root,
            resources: bundled_resources,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn catalog(&self) -> Vec<ToolPackage> {
        let bundled = self.bundled().unwrap_or_default();
        RECIPES
            .iter()
            .map(|r| ToolPackage {
                id: r.id.into(),
                name: r.name.into(),
                languages: r.languages.iter().map(|l| (*l).into()).collect(),
                source_url: r.source.into(),
                license: r.license.into(),
                distribution: if bundled.contains_key(r.id) {
                    ToolDistribution::Bundled
                } else {
                    ToolDistribution::FirstUseDownload
                },
                prerequisites: vec![],
                limitations: r.limitations.iter().map(|s| (*s).into()).collect(),
            })
            .collect()
    }
    fn manifest(&self) -> Result<Manifest> {
        let path = self.root.join("managed-tools.json");
        match fs::read(&path) {
            Ok(bytes) => {
                let manifest: Manifest =
                    serde_json::from_slice(&bytes).context("Invalid managed tool manifest")?;
                if manifest.schema_version != 1 {
                    bail!("Unsupported managed tool manifest version");
                }
                Ok(manifest)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest {
                schema_version: 1,
                ..Default::default()
            }),
            Err(e) => Err(e.into()),
        }
    }
    fn bundled(&self) -> Result<BTreeMap<String, InstalledTool>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Bundle {
            schema_version: u32,
            packages: Vec<InstalledTool>,
        }
        let Some(resources) = &self.resources else {
            return Ok(BTreeMap::new());
        };
        let path = resources.join("language-tools/manifest.json");
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(e.into()),
        };
        let bundle: Bundle =
            serde_json::from_slice(&bytes).context("Invalid bundled language-tool manifest")?;
        if bundle.schema_version != 1 {
            bail!("Unsupported bundled tools version");
        }
        let canonical = resources.canonicalize()?;
        let mut result = BTreeMap::new();
        for mut tool in bundle.packages {
            if tool.package == "kotlin" {
                bail!(
                    "Kotlin bundle rejected: complete binary redistribution grant has not been established"
                );
            }
            tool.distribution = ToolDistribution::Bundled;
            for c in &mut tool.servers {
                c.program = resource_value(&c.program, resources, &canonical, true)?;
                for arg in &mut c.args {
                    *arg = resource_value(arg, resources, &canonical, false)?;
                }
            }
            for c in &mut tool.formatters {
                c.program = resource_value(&c.program, resources, &canonical, true)?;
                for arg in &mut c.args {
                    *arg = resource_value(arg, resources, &canonical, false)?;
                }
            }
            for notice in &mut tool.notices {
                *notice = resource_value(notice, resources, &canonical, false)?;
            }
            validate_tool(&tool, &canonical, true)?;
            result.insert(tool.package.clone(), tool);
        }
        Ok(result)
    }
    pub fn status(&self, package: &str) -> Result<ToolStatus> {
        if !RECIPES.iter().any(|r| r.id == package) {
            bail!("Unknown language package {package}");
        }
        let mut installed = self.manifest()?.packages.remove(package);
        if installed.is_none() {
            installed = self.bundled()?.remove(package);
        }
        let problem = installed.as_ref().and_then(|tool| {
            let base = if tool.distribution == ToolDistribution::Bundled {
                self.resources.as_deref().unwrap_or(&self.root)
            } else {
                &self.root
            };
            validate_tool(tool, base, true)
                .err()
                .map(|e| format!("{e:#}"))
        });
        Ok(ToolStatus {
            package: package.into(),
            installed,
            problem,
        })
    }
    /// Last durable terminal install/update result. Does not resume an interrupted download.
    pub fn last_attempt(&self, package: &str) -> Result<Option<InstallProgress>> {
        if !RECIPES.iter().any(|r| r.id == package) {
            bail!("Unknown language package");
        }
        let path = self.root.join("attempts").join(format!("{package}.json"));
        let mut progress: InstallProgress = match fs::read(path) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if !matches!(
            progress.phase,
            InstallPhase::Complete | InstallPhase::Failed | InstallPhase::Cancelled
        ) {
            progress.phase = InstallPhase::Failed;
            progress.message =
                "Previous installation was interrupted; active version retained. Retry explicitly."
                    .into();
            progress.error = Some(progress.message.clone());
        }
        Ok(Some(progress))
    }
    pub fn statuses(&self) -> Result<Vec<ToolStatus>> {
        RECIPES.iter().map(|r| self.status(r.id)).collect()
    }
    pub fn server_configs(&self) -> Result<Vec<ServerConfig>> {
        Ok(self
            .statuses()?
            .into_iter()
            .filter(|s| s.problem.is_none())
            .filter_map(|s| s.installed)
            .flat_map(|s| s.servers)
            .collect())
    }
    pub fn formatter_configs(&self) -> Result<Vec<FormatterConfig>> {
        Ok(self
            .statuses()?
            .into_iter()
            .filter(|s| s.problem.is_none())
            .filter_map(|s| s.installed)
            .flat_map(|s| s.formatters)
            .collect())
    }
    pub fn configs_with_overrides(&self, custom: &[ServerConfig]) -> Result<Vec<ServerConfig>> {
        let mut configs = self.server_configs()?;
        for c in custom {
            configs.retain(|old| {
                crate::config::normalize_language(&old.language)
                    != crate::config::normalize_language(&c.language)
            });
            configs.push(c.clone());
        }
        Ok(configs)
    }
    pub fn begin_install(&self, package: &str) -> Result<InstallControl> {
        let _recipe = RECIPES
            .iter()
            .find(|r| r.id == package)
            .context("Unknown language package")?;

        let control = InstallControl::new(package);
        let returned = control.clone();
        let tools = self.clone();
        let package = package.to_owned();
        thread::Builder::new()
            .name(format!("install-{package}"))
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tools.install(&package, &control)
                }))
                .unwrap_or_else(|_| {
                    Err(anyhow::anyhow!(
                        "Installer worker panicked; previous active version retained"
                    ))
                });
                control.finish(outcome);
                let attempts = tools.root.join("attempts");
                if attempts.is_dir() {
                    let _ = atomic_json(
                        &attempts.join(format!("{package}.json")),
                        &control.snapshot(),
                    );
                }
            })?;
        Ok(returned)
    }
    pub fn begin_update(&self, package: &str) -> Result<InstallControl> {
        self.begin_install(package)
    }
    fn install(&self, package: &str, control: &InstallControl) -> Result<InstalledTool> {
        control.check()?;
        create_owned_root(&self.root)?;
        fs::create_dir_all(self.root.join("attempts"))?;
        atomic_json(
            &self.root.join("attempts").join(format!("{package}.json")),
            &control.snapshot(),
        )?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join("install.lock"))?;
        loop {
            match fs2::FileExt::try_lock_exclusive(&lock) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    control.check()?;
                    thread::sleep(std::time::Duration::from_millis(40));
                }
                Err(e) => return Err(e.into()),
            }
        }
        let scratch = tempfile::Builder::new()
            .prefix("install-")
            .tempdir_in(&self.root)?;
        let payload = scratch.path().join("payload");
        fs::create_dir(&payload)?;
        control.phase(
            InstallPhase::Resolving,
            "Resolving official release metadata",
        );
        let mut tool = recipes::install(package, &payload, scratch.path(), control)?;
        control.check()?;
        let destination_parent = self.root.join("packages").join(package);
        fs::create_dir_all(&destination_parent)?;
        let destination = destination_parent.join(format!(
            "{}-{}",
            safe_name(&tool.version)?,
            scratch.path().file_name().unwrap().to_string_lossy()
        ));
        // Configs contain staging absolutes until this point; rewrite to final immutable paths.
        for config in &mut tool.servers {
            config.program = relocate(&config.program, &payload, &destination);
            for a in &mut config.args {
                *a = relocate(a, &payload, &destination);
            }
        }
        for config in &mut tool.formatters {
            config.program = relocate(&config.program, &payload, &destination);
            for a in &mut config.args {
                *a = relocate(a, &payload, &destination);
            }
        }
        for notice in &mut tool.notices {
            *notice = relocate(notice, &payload, &destination);
        }
        self.activate_prepared(package, &tool, &payload, &destination, control)
    }
    fn activate_prepared(
        &self,
        package: &str,
        tool: &InstalledTool,
        payload: &Path,
        destination: &Path,
        control: &InstallControl,
    ) -> Result<InstalledTool> {
        control.phase(
            InstallPhase::Activating,
            "Atomically activating verified tools",
        );
        let _guard = control
            .inner
            .activation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        control.check()?;
        fs::rename(payload, destination)?;
        let activated = (|| -> Result<()> {
            validate_tool(tool, &self.root, true)?;
            let mut manifest = self.manifest()?;
            manifest.packages.insert(package.into(), tool.clone());
            atomic_json(&self.root.join("managed-tools.json"), &manifest)?;
            Ok(())
        })();
        if let Err(error) = activated {
            let _ = fs::remove_dir_all(destination);
            return Err(error);
        }
        control.inner.committed.store(true, Ordering::Release);
        // Keep previous immutable versions for rollback and already-running sessions.
        Ok(tool.clone())
    }
}
fn relocate(value: &str, from: &Path, to: &Path) -> String {
    value.replace(
        from.to_string_lossy().as_ref(),
        to.to_string_lossy().as_ref(),
    )
}
fn safe_name(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
    {
        bail!("Unsafe release version");
    }
    Ok(value.into())
}
fn validate_absolute(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("Managed tool paths must be absolute without parent traversal");
    }
    let mut p = PathBuf::new();
    for part in path.components() {
        p.push(part);
        if let Ok(meta) = fs::symlink_metadata(&p)
            && meta.file_type().is_symlink()
        {
            bail!(
                "Managed tool root must not use symlink ancestors: {}",
                p.display()
            );
        }
    }
    Ok(())
}
fn create_owned_root(path: &Path) -> Result<()> {
    validate_absolute(path)?;
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn resource_value(
    value: &str,
    resources: &Path,
    canonical: &Path,
    program: bool,
) -> Result<String> {
    let result = value.replace("{resources}", resources.to_string_lossy().as_ref());
    if value.contains("{resources}") {
        // Path may follow an argv env assignment or a --name= option.
        let paths = if let Some((_, p)) = result.split_once('=') {
            p
        } else {
            &result
        };
        for item in paths.split(':') {
            if item.contains(resources.to_string_lossy().as_ref()) {
                let path = Path::new(item);
                if !path.is_absolute() || !path.canonicalize()?.starts_with(canonical) {
                    bail!("Bundled tool path escapes Resources");
                }
            }
        }
    } else if program && result != "/usr/bin/env" {
        bail!("Bundled executable must be Resources-relative");
    }
    Ok(result)
}
fn validate_tool(tool: &InstalledTool, base: &Path, check_files: bool) -> Result<()> {
    safe_name(&tool.package)?;
    safe_name(&tool.version)?;
    if tool.artifacts.is_empty()
        || tool
            .artifacts
            .iter()
            .any(|a| a.version.is_empty() || a.source_url.is_empty() || a.checksum.is_empty())
    {
        bail!("Tool manifest lacks exact artifact provenance");
    }
    for c in &tool.servers {
        validate_command(&c.program, &c.args, base, check_files)?;
    }
    for c in &tool.formatters {
        validate_command(&c.program, &c.args, base, check_files)?;
    }
    Ok(())
}
fn validate_command(program: &str, args: &[String], base: &Path, check_files: bool) -> Result<()> {
    let base = base.canonicalize()?;
    let path = Path::new(program);
    if !path.is_absolute() {
        bail!("Managed executable is not absolute");
    }
    if program != "/usr/bin/env" && !path.canonicalize()?.starts_with(&base) {
        bail!("Managed executable escapes its installation root");
    }
    if check_files && crate::discover_program(program).is_none() {
        bail!("Installed executable unavailable: {program}");
    }
    if program == "/usr/bin/env" {
        let payload = args
            .iter()
            .find(|s| !s.contains('=') && !s.starts_with('-'))
            .context("Managed env launcher lacks a payload")?;
        let p = Path::new(payload);
        if !p.is_absolute()
            || !p.canonicalize()?.starts_with(&base)
            || crate::discover_program(payload).is_none()
        {
            bail!("Managed launcher payload unavailable or outside its root");
        }
    }
    for arg in args {
        if arg.starts_with('/') && !arg.contains("{workspace}") && !arg.contains("{root}") {
            for item in arg.split(':') {
                let path = Path::new(item);
                if !path
                    .canonicalize()
                    .with_context(|| format!("Installed payload unavailable: {item}"))?
                    .starts_with(&base)
                {
                    bail!("Installed payload escapes tool root");
                }
            }
        }
    }
    Ok(())
}
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    use std::io::Write;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().context("Manifest requires parent")?)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    // The rename is the commit point. A directory-fsync error after it must not
    // make callers delete the now-active payload. File data was synced above.
    if let Ok(parent) = fs::File::open(path.parent().unwrap()) {
        let _ = parent.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn tool(root: &Path, version: &str) -> InstalledTool {
        let bin = root.join(format!("server-{version}"));
        fs::write(&bin, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        InstalledTool {
            package: "python".into(),
            version: version.into(),
            distribution: ToolDistribution::FirstUseDownload,
            artifacts: vec![ToolArtifact {
                version: version.into(),
                source_url: "https://example.com/official".into(),
                checksum: format!("sha256:{}", "a".repeat(64)),
                metadata_url: "https://example.com/release".into(),
            }],
            servers: vec![ServerConfig {
                language: "python".into(),
                program: bin.to_string_lossy().into_owned(),
                args: vec![],
            }],
            formatters: vec![],
            notices: vec![],
        }
    }
    #[test]
    fn construction_and_discovery_do_not_create_or_execute() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap().join("uncreated");
        let tools = ManagedTools::new(root.clone(), None).unwrap();
        assert!(!root.exists());
        assert!(tools.status("python").unwrap().installed.is_none());
        assert!(tools.server_configs().unwrap().is_empty());
        assert!(!root.exists());
        assert!(ManagedTools::new(PathBuf::from("relative"), None).is_err());
        assert!(tools.begin_install("../../../bad").is_err());
    }
    #[test]
    fn atomic_manifest_updates_keep_previous_version_on_failure_and_custom_priority() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let tools = ManagedTools::new(root.clone(), None).unwrap();
        let old = tool(&root, "1.0");
        let mut manifest = Manifest {
            schema_version: 1,
            ..Default::default()
        };
        manifest.packages.insert("python".into(), old.clone());
        atomic_json(&root.join("managed-tools.json"), &manifest).unwrap();
        assert_eq!(
            tools.status("python").unwrap().installed.unwrap().version,
            "1.0"
        );
        let next = tool(&root, "2.0");
        fs::remove_file(&next.servers[0].program).unwrap();
        assert!(validate_tool(&next, &root, true).is_err());
        assert_eq!(
            tools.status("python").unwrap().installed.unwrap().version,
            "1.0"
        );
        let custom = ServerConfig {
            language: "python".into(),
            program: "/explicit/custom/python-server".into(),
            args: vec!["--custom".into()],
        };
        let configs = tools
            .configs_with_overrides(std::slice::from_ref(&custom))
            .unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].program, custom.program);
        fs::remove_file(&old.servers[0].program).unwrap();
        let status = tools.status("python").unwrap();
        assert!(status.installed.is_some());
        assert!(status.problem.is_some());
        assert!(tools.server_configs().unwrap().is_empty());
    }
    #[test]
    fn activation_update_failure_rolls_back_and_success_retains_previous_version() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let tools = ManagedTools::new(root.clone(), None).unwrap();
        let old = tool(&root, "1.0");
        let mut manifest = Manifest {
            schema_version: 1,
            ..Default::default()
        };
        manifest.packages.insert("python".into(), old.clone());
        atomic_json(&root.join("managed-tools.json"), &manifest).unwrap();
        let before = fs::read(root.join("managed-tools.json")).unwrap();
        let stage = root.join("stage");
        fs::create_dir(&stage).unwrap();
        let mut next = tool(&stage, "2.0");
        let destination = root.join("version-2");
        next.servers[0].program = relocate(&next.servers[0].program, &stage, &destination);
        fs::remove_file(stage.join("server-2.0")).unwrap();
        assert!(
            tools
                .activate_prepared(
                    "python",
                    &next,
                    &stage,
                    &destination,
                    &InstallControl::new("python")
                )
                .is_err()
        );
        assert!(!destination.exists());
        assert_eq!(fs::read(root.join("managed-tools.json")).unwrap(), before);
        assert!(Path::new(&old.servers[0].program).exists());
        fs::create_dir(&stage).unwrap();
        let mut next = tool(&stage, "2.0");
        next.servers[0].program = relocate(&next.servers[0].program, &stage, &destination);
        tools
            .activate_prepared(
                "python",
                &next,
                &stage,
                &destination,
                &InstallControl::new("python"),
            )
            .unwrap();
        assert_eq!(
            tools.status("python").unwrap().installed.unwrap().version,
            "2.0"
        );
        assert!(Path::new(&old.servers[0].program).exists());
    }
    #[test]
    fn public_update_cancel_while_locked_retains_active_install() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let tools = ManagedTools::new(root.clone(), None).unwrap();
        let old = tool(&root, "1.0");
        let mut manifest = Manifest {
            schema_version: 1,
            ..Default::default()
        };
        manifest.packages.insert("python".into(), old);
        atomic_json(&root.join("managed-tools.json"), &manifest).unwrap();
        let before = fs::read(root.join("managed-tools.json")).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("install.lock"))
            .unwrap();
        fs2::FileExt::lock_exclusive(&lock).unwrap();
        let update = tools.begin_update("python").unwrap();
        update.cancel().unwrap();
        assert!(update.wait().is_err());
        assert_eq!(update.snapshot().phase, InstallPhase::Cancelled);
        assert_eq!(fs::read(root.join("managed-tools.json")).unwrap(), before);
        fs2::FileExt::unlock(&lock).unwrap();
    }
    #[test]
    fn bundle_requires_internal_resources_and_preserves_distribution() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let resources = root.join("Resources");
        fs::create_dir_all(resources.join("language-tools")).unwrap();
        let mut bundle = tool(&resources, "1.0");
        bundle.package = "java".into();
        bundle.servers[0].language = "java".into();
        bundle.servers[0].program = "{resources}/server-1.0".into();
        atomic_json(
            &resources.join("language-tools/manifest.json"),
            &serde_json::json!({"schemaVersion":1,"packages":[bundle]}),
        )
        .unwrap();
        let tools = ManagedTools::new(root.join("state"), Some(resources.clone())).unwrap();
        assert_eq!(
            tools
                .status("java")
                .unwrap()
                .installed
                .unwrap()
                .distribution,
            ToolDistribution::Bundled
        );
        assert!(!root.join("state").exists());
        assert!(resource_value("{resources}/../../escape", &resources, &resources, true).is_err());
        let mut bundle = tool(&resources, "2.0");
        bundle.package = "kotlin".into();
        bundle.servers[0].program = "{resources}/server-2.0".into();
        atomic_json(
            &resources.join("language-tools/manifest.json"),
            &serde_json::json!({"schemaVersion":1,"packages":[bundle]}),
        )
        .unwrap();
        assert!(tools.status("kotlin").is_err());
    }
    #[test]
    fn cancelled_job_does_not_erase_working_manifest() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let tools = ManagedTools::new(root.clone(), None).unwrap();
        let old = tool(&root, "1.0");
        let mut manifest = Manifest {
            schema_version: 1,
            ..Default::default()
        };
        manifest.packages.insert("python".into(), old);
        atomic_json(&root.join("managed-tools.json"), &manifest).unwrap();
        let before = fs::read(root.join("managed-tools.json")).unwrap();
        let c = InstallControl::new("python");
        c.cancel().unwrap();
        assert!(tools.install("python", &c).is_err());
        assert_eq!(fs::read(root.join("managed-tools.json")).unwrap(), before);
    }
}
