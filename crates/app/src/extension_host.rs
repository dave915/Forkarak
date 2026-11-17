//! Versioned local extension registry and lazy, owned JSON-lines subprocess host.
//! Isolation is a process boundary, not a filesystem/security sandbox. Local code runs
//! only after the user enables it and explicitly invokes a capability in a trusted tree.
use crate::task_runtime::{LogBuffer, LogOptions, OwnedProcess, StreamKind, TaskProfile};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

pub const HOST_API_VERSION: &str = "1.0.0";
pub const SPRING_ID: &str = "forkarak.spring";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandRegistration {
    pub id: String,
    pub title: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingRegistration {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub default: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolRegistration {
    #[serde(default)]
    pub extension_id: String,
    pub language: String,
    /// A bundle-relative path in manifests, an absolute path in discovery results.
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub file_extensions: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CapabilityRegistration {
    #[serde(default)]
    pub commands: Vec<CommandRegistration>,
    #[serde(default)]
    pub settings: Vec<SettingRegistration>,
    #[serde(default)]
    pub formatters: Vec<ToolRegistration>,
    #[serde(default)]
    pub language_servers: Vec<ToolRegistration>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub api_version: String,
    pub id: String,
    pub name: String,
    pub version: String,
    /// Bundle-relative executable. Builtin Spring is resolved by the host itself.
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub capabilities: CapabilityRegistration,
}
fn semver(value: &str) -> Result<(u32, u32, u32)> {
    let parts = value
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        parts.len() == 3,
        "버전은 major.minor.patch 형식이어야 합니다"
    );
    Ok((parts[0], parts[1], parts[2]))
}
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        && !id.starts_with('.')
}
fn relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}
fn bundled_path(bundle: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(
        relative(path),
        "확장 경로는 번들 내부의 상대 경로여야 합니다"
    );
    let base = bundle.canonicalize()?;
    let path = base.join(path).canonicalize()?;
    ensure!(
        path.starts_with(&base) && path.is_file(),
        "확장 파일이 번들 밖을 가리키거나 없습니다"
    );
    Ok(path)
}
impl ExtensionManifest {
    pub fn validate(&self, bundle: &Path) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "지원하지 않는 확장 manifest 버전입니다"
        );
        let (major, minor, _) = semver(&self.api_version)?;
        ensure!(
            major == 1 && minor == 0,
            "확장 API {}는 호스트 {}와 호환되지 않습니다",
            self.api_version,
            HOST_API_VERSION
        );
        ensure!(
            safe_id(&self.id) && self.id != SPRING_ID,
            "올바른 확장 ID를 지정하세요 (내장 ID는 예약됨)"
        );
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 256,
            "확장 이름을 확인하세요"
        );
        semver(&self.version)?;
        let executable = bundled_path(bundle, &self.executable)?;
        executable_file(&executable)?;
        let mut ids = std::collections::HashSet::new();
        for cmd in &self.capabilities.commands {
            ensure!(
                safe_id(&cmd.id) && !cmd.title.trim().is_empty() && ids.insert(cmd.id.clone()),
                "확장 명령 ID/이름이 잘못되었거나 중복됩니다"
            );
        }
        let mut keys = std::collections::HashSet::new();
        for setting in &self.capabilities.settings {
            ensure!(
                !setting.key.trim().is_empty() && keys.insert(&setting.key),
                "설정 키가 비어 있거나 중복됩니다"
            );
        }
        for tool in self
            .capabilities
            .formatters
            .iter()
            .chain(&self.capabilities.language_servers)
        {
            ensure!(!tool.language.is_empty(), "언어 이름을 지정하세요");
            executable_file(&bundled_path(bundle, &tool.executable)?)?;
        }
        ensure!(
            self.args.iter().all(|a| !a.contains('\0')),
            "NUL 인수를 사용할 수 없습니다"
        );
        Ok(())
    }
}
fn executable_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        fs::metadata(path)?.permissions().mode() & 0o111 != 0,
        "실행 권한이 없습니다: {}",
        path.display()
    );
    Ok(())
}
#[derive(Clone, Debug)]
pub struct InstalledExtension {
    pub manifest: ExtensionManifest,
    pub bundle: PathBuf,
    pub builtin: bool,
    pub enabled: bool,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct RegistryState {
    enabled: HashMap<String, bool>,
}
pub struct ExtensionRegistry {
    pub directory: PathBuf,
    state: RegistryState,
}
impl ExtensionRegistry {
    pub fn new(directory: PathBuf) -> Self {
        let state = fs::read(directory.join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self { directory, state }
    }
    fn save(&self) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer_pretty(&mut tmp, &self.state)?;
        tmp.persist(self.directory.join("state.json"))
            .map_err(|e| e.error)?;
        Ok(())
    }
    pub fn list(&self) -> Vec<InstalledExtension> {
        let mut list = vec![InstalledExtension {
            manifest: spring_manifest(),
            bundle: PathBuf::new(),
            builtin: true,
            enabled: *self.state.enabled.get(SPRING_ID).unwrap_or(&true),
        }];
        if let Ok(entries) = fs::read_dir(&self.directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_dir())
                    && let Ok(bytes) = fs::read(path.join("manifest.json"))
                    && let Ok(manifest) = serde_json::from_slice::<ExtensionManifest>(&bytes)
                    && path.file_name().and_then(|s| s.to_str()) == Some(&manifest.id)
                    && manifest.validate(&path).is_ok()
                {
                    let enabled = *self.state.enabled.get(&manifest.id).unwrap_or(&false);
                    list.push(InstalledExtension {
                        manifest,
                        bundle: path,
                        builtin: false,
                        enabled,
                    });
                }
            }
        }
        list.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        list
    }
    pub fn get(&self, id: &str) -> Result<InstalledExtension> {
        self.list()
            .into_iter()
            .find(|e| e.manifest.id == id)
            .context("설치된 확장을 찾을 수 없습니다")
    }
    /// Copy a validated local bundle. An update must keep its ID and increase SemVer.
    pub fn install(&mut self, source: &Path, update: bool) -> Result<String> {
        let (manifest_path, bundle) = if source.is_dir() {
            (source.join("manifest.json"), source.to_path_buf())
        } else {
            (
                source.to_path_buf(),
                source
                    .parent()
                    .context("번들 폴더가 없습니다")?
                    .to_path_buf(),
            )
        };
        ensure!(
            fs::metadata(&manifest_path)?.len() <= 256 * 1024,
            "manifest가 너무 큽니다"
        );
        let manifest: ExtensionManifest = serde_json::from_slice(&fs::read(manifest_path)?)?;
        manifest.validate(&bundle)?;
        fs::create_dir_all(&self.directory)?;
        let base = self.directory.canonicalize()?;
        let bundle = bundle.canonicalize()?;
        ensure!(
            !base.starts_with(&bundle) && !bundle.starts_with(&base),
            "설치 디렉터리 자체를 번들로 사용할 수 없습니다"
        );
        let destination = self.directory.join(&manifest.id);
        let old = self.get(&manifest.id).ok();
        if let Some(old) = old {
            ensure!(update, "이미 설치된 확장입니다. 업데이트를 선택하세요");
            ensure!(
                semver(&manifest.version)? > semver(&old.manifest.version)?,
                "업데이트 버전은 현재 버전보다 높아야 합니다"
            );
        } else {
            ensure!(!update, "업데이트할 확장이 없습니다");
            ensure!(!destination.exists(), "기존 설치 폴더를 확인하세요");
        }
        let stage = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&self.directory)?;
        let mut budget = (0usize, 0u64);
        copy_bundle(&bundle, stage.path(), &mut budget)?;
        fs::write(
            stage.path().join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        manifest.validate(stage.path())?;
        let backup = self.directory.join(format!(".backup-{}", manifest.id));
        ensure!(!backup.exists(), "이전 업데이트 백업을 먼저 확인하세요");
        if destination.exists() {
            fs::rename(&destination, &backup)?;
        }
        if let Err(e) = fs::rename(stage.path(), &destination) {
            if backup.exists() {
                let _ = fs::rename(&backup, &destination);
            }
            return Err(e.into());
        }
        if backup.exists() {
            fs::remove_dir_all(backup)?;
        }
        self.state
            .enabled
            .entry(manifest.id.clone())
            .or_insert(false);
        self.save()?;
        Ok(manifest.id)
    }
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<()> {
        self.get(id)?;
        self.state.enabled.insert(id.into(), enabled);
        self.save()
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let ext = self.get(id)?;
        ensure!(!ext.builtin, "내장 확장은 비활성화할 수 있습니다");
        fs::remove_dir_all(ext.bundle)?;
        self.state.enabled.remove(id);
        self.save()
    }
    /// Pure discovery. No process startup; callers still enforce analysis trust.
    pub fn language_servers(&self, overrides: &HashMap<String, bool>) -> Vec<ToolRegistration> {
        self.tools(overrides, false)
    }
    pub fn formatters(&self, overrides: &HashMap<String, bool>) -> Vec<ToolRegistration> {
        self.tools(overrides, true)
    }
    fn tools(&self, overrides: &HashMap<String, bool>, formatter: bool) -> Vec<ToolRegistration> {
        self.list()
            .into_iter()
            .filter(|e| e.enabled && overrides.get(&e.manifest.id).copied().unwrap_or(true))
            .flat_map(|e| {
                let tools = if formatter {
                    e.manifest.capabilities.formatters
                } else {
                    e.manifest.capabilities.language_servers
                };
                tools.into_iter().filter_map(move |mut t| {
                    t.executable = bundled_path(&e.bundle, &t.executable).ok()?;
                    t.extension_id = e.manifest.id.clone();
                    Some(t)
                })
            })
            .collect()
    }
}
fn copy_bundle(source: &Path, target: &Path, budget: &mut (usize, u64)) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "번들에는 심볼릭 링크를 사용할 수 없습니다"
        );
        budget.0 += 1;
        budget.1 += metadata.len();
        ensure!(
            budget.0 <= 5000 && budget.1 <= 64 * 1024 * 1024,
            "로컬 확장 번들 한도(5000 파일, 64 MiB)를 초과했습니다"
        );
        let out = target.join(entry.file_name());
        if metadata.is_dir() {
            fs::create_dir(&out)?;
            copy_bundle(&entry.path(), &out, budget)?;
        } else {
            ensure!(metadata.is_file(), "일반 파일과 폴더만 설치할 수 있습니다");
            fs::copy(entry.path(), out)?;
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtensionRequest {
    pub api_version: String,
    pub request_id: u64,
    pub generation: u64,
    pub worktree: PathBuf,
    pub command: String,
    #[serde(default)]
    pub settings: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtensionResponse {
    pub api_version: String,
    pub request_id: u64,
    pub generation: u64,
    pub worktree: PathBuf,
    #[serde(default)]
    pub tasks: Vec<TaskProfile>,
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}
struct Session {
    process: OwnedProcess,
    requests: mpsc::Sender<Vec<u8>>,
    written: Receiver<Result<()>>,
    responses: Receiver<Result<ExtensionResponse>>,
    generation: u64,
}
impl Session {
    fn spawn(
        extension: &InstalledExtension,
        root: &Path,
        logs: Arc<Mutex<LogBuffer>>,
        generation: u64,
    ) -> Result<Self> {
        let mut cmd = if extension.builtin {
            let mut cmd = Command::new(std::env::current_exe()?);
            cmd.args(["--extension-host", "spring"]);
            cmd
        } else {
            extension.manifest.validate(&extension.bundle)?;
            let mut cmd = Command::new(bundled_path(
                &extension.bundle,
                &extension.manifest.executable,
            )?);
            cmd.args(&extension.manifest.args);
            cmd
        };
        cmd.current_dir(root)
            .env("FORKARAK_EXTENSION_API", HOST_API_VERSION);
        let (process, stdin, stdout) = OwnedProcess::spawn(cmd, logs, false)?;
        let (tx, responses) = mpsc::sync_channel(8);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout.expect("piped stdout"));
            loop {
                let mut bytes = Vec::new();
                let count = (&mut reader)
                    .take(256 * 1024 + 1)
                    .read_until(b'\n', &mut bytes);
                let response = match count {
                    Ok(0) => break,
                    Ok(n) if n <= 256 * 1024 => {
                        serde_json::from_slice(&bytes).context("확장 응답 JSON 오류")
                    }
                    _ => Err(anyhow::anyhow!("확장 응답 한도를 초과했습니다")),
                };
                let failed = response.is_err();
                if tx.send(response).is_err() || failed {
                    break;
                }
            }
        });
        let (requests, request_rx) = mpsc::channel::<Vec<u8>>();
        let (write_tx, written) = mpsc::channel();
        thread::spawn(move || {
            let mut stdin = stdin.expect("piped stdin");
            while let Ok(message) = request_rx.recv() {
                let result = stdin
                    .write_all(&message)
                    .and_then(|_| stdin.flush())
                    .map_err(anyhow::Error::from);
                let failed = result.is_err();
                if write_tx.send(result).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self {
            process,
            requests,
            written,
            responses,
            generation,
        })
    }
    fn call(&mut self, request: &ExtensionRequest) -> Result<ExtensionResponse> {
        ensure!(
            self.process.status().active(),
            "확장 프로세스가 종료되었습니다. 다시 실행하세요"
        );
        let mut message = serde_json::to_vec(request)?;
        message.push(b'\n');
        ensure!(message.len() <= 256 * 1024, "확장 요청이 너무 큽니다");
        self.requests
            .send(message)
            .context("확장 입력 채널이 닫혔습니다")?;
        self.written
            .recv_timeout(Duration::from_secs(8))
            .context("확장 입력 전송 시간이 초과되었습니다")??;
        let response = self
            .responses
            .recv_timeout(Duration::from_secs(8))
            .context("확장이 8초 안에 응답하지 않았습니다")??;
        ensure!(
            response.request_id == request.request_id
                && response.generation == request.generation
                && response.worktree == request.worktree
                && self.generation == request.generation,
            "오래되었거나 다른 워크트리의 확장 응답을 거부했습니다"
        );
        let (major, minor, _) = semver(&response.api_version)?;
        ensure!(
            major == 1 && minor == 0,
            "호환되지 않는 확장 응답 API입니다"
        );
        ensure!(
            response.tasks.len() <= 50 && response.logs.len() <= 1000,
            "확장 결과 한도를 초과했습니다"
        );
        if let Some(error) = &response.error {
            bail!("{error}");
        }
        for profile in &response.tasks {
            profile.validate(&request.worktree)?;
        }
        Ok(response)
    }
}
type Key = (PathBuf, String);
struct SessionSlot {
    inner: Mutex<Session>,
    cancelled: Arc<AtomicBool>,
    status: Arc<Mutex<crate::task_runtime::RunStatus>>,
}
impl SessionSlot {
    fn stop(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
impl Drop for SessionSlot {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
pub struct ExtensionHost {
    sessions: Mutex<HashMap<Key, Arc<SessionSlot>>>,
    logs: Mutex<HashMap<Key, Arc<Mutex<LogBuffer>>>>,
    next: AtomicU64,
}
impl ExtensionHost {
    pub fn logs(&self, root: &Path, id: &str) -> Option<Arc<Mutex<LogBuffer>>> {
        self.logs
            .lock()
            .unwrap()
            .get(&(root.to_path_buf(), id.into()))
            .cloned()
    }
    pub fn active(&self, root: &Path, id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(&(root.to_path_buf(), id.into()))
            .is_some_and(|slot| {
                !slot.cancelled.load(Ordering::Acquire) && slot.status.lock().unwrap().active()
            })
    }
    /// Call on a background executor. No activation before an explicit trusted capability use.
    pub fn invoke(
        &self,
        extension: &InstalledExtension,
        root: &Path,
        command: &str,
        settings: Value,
        trusted: bool,
        project_enabled: bool,
        options: LogOptions,
    ) -> Result<ExtensionResponse> {
        ensure!(trusted, "이 워크트리의 실행을 먼저 허용하세요");
        ensure!(
            extension.enabled && project_enabled,
            "이 확장은 비활성화되어 있습니다"
        );
        ensure!(
            extension
                .manifest
                .capabilities
                .commands
                .iter()
                .any(|c| c.id == command),
            "등록되지 않은 확장 명령입니다"
        );
        let root = root.canonicalize()?;
        let key = (root.clone(), extension.manifest.id.clone());
        let request_id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let logs = self
            .logs
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(LogBuffer::new(options))))
            .clone();
        let slot = {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(session) = sessions.get(&key).filter(|slot| {
                !slot.cancelled.load(Ordering::Acquire) && slot.status.lock().unwrap().active()
            }) {
                session.clone()
            } else {
                sessions.remove(&key);
                let session = Session::spawn(extension, &root, logs.clone(), request_id)?;
                let cancelled = session.process.cancellation();
                let status = session.process.status.clone();
                let session = Arc::new(SessionSlot {
                    inner: Mutex::new(session),
                    cancelled,
                    status,
                });
                sessions.insert(key.clone(), session.clone());
                logs.lock()
                    .unwrap()
                    .push(StreamKind::Host, "명시적 명령으로 확장을 활성화했습니다");
                session
            }
        };
        let mut session = slot.inner.lock().unwrap();
        ensure!(
            !slot.cancelled.load(Ordering::Acquire),
            "확장 실행이 취소되었습니다"
        );
        let request = ExtensionRequest {
            api_version: HOST_API_VERSION.into(),
            request_id,
            generation: session.generation,
            worktree: root,
            command: command.into(),
            settings,
        };
        match session.call(&request) {
            Ok(response) => {
                ensure!(
                    !slot.cancelled.load(Ordering::Acquire)
                        && self
                            .sessions
                            .lock()
                            .unwrap()
                            .get(&key)
                            .is_some_and(|current| Arc::ptr_eq(current, &slot)),
                    "비활성화된 확장의 오래된 응답을 거부했습니다"
                );
                for line in &response.logs {
                    logs.lock().unwrap().push(StreamKind::Host, line);
                }
                Ok(response)
            }
            Err(error) => {
                session.process.request_stop();
                let mut sessions = self.sessions.lock().unwrap();
                if sessions
                    .get(&key)
                    .is_some_and(|current| Arc::ptr_eq(current, &slot))
                {
                    sessions.remove(&key);
                }
                drop(sessions);
                logs.lock()
                    .unwrap()
                    .push(StreamKind::Host, error.to_string());
                Err(error)
            }
        }
    }
    pub fn deactivate(&self, root: &Path, id: &str) {
        if let Some(slot) = self
            .sessions
            .lock()
            .unwrap()
            .remove(&(root.to_path_buf(), id.into()))
        {
            slot.stop();
        }
    }
    pub fn deactivate_extension(&self, id: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        for ((_, extension), slot) in sessions.iter() {
            if extension == id {
                slot.stop();
            }
        }
        sessions.retain(|(_, extension), _| extension != id);
    }
    pub fn shutdown_root(&self, root: &Path) {
        self.sessions.lock().unwrap().retain(|(path, _), slot| {
            if path == root {
                slot.stop();
                false
            } else {
                true
            }
        });
    }
    pub fn shutdown(&self) {
        let mut sessions = self.sessions.lock().unwrap();
        for slot in sessions.values() {
            slot.stop();
        }
        sessions.clear();
    }
}

pub fn spring_manifest() -> ExtensionManifest {
    ExtensionManifest {
        schema_version: 1,
        api_version: HOST_API_VERSION.into(),
        id: SPRING_ID.into(),
        name: "Spring".into(),
        version: "1.0.0".into(),
        executable: "builtin-spring".into(),
        args: vec![],
        capabilities: CapabilityRegistration {
            commands: vec![CommandRegistration {
                id: "profiles".into(),
                title: "Spring 실행 프로필 만들기".into(),
            }],
            settings: vec![
                SettingRegistration {
                    key: "spring.jdk".into(),
                    title: "JDK 홈".into(),
                    default: json!(""),
                },
                SettingRegistration {
                    key: "spring.gradle".into(),
                    title: "Gradle 실행 파일".into(),
                    default: json!(""),
                },
                SettingRegistration {
                    key: "spring.maven".into(),
                    title: "Maven 실행 파일".into(),
                    default: json!(""),
                },
                SettingRegistration {
                    key: "spring.args".into(),
                    title: "추가 인수".into(),
                    default: json!([]),
                },
                SettingRegistration {
                    key: "spring.profiles".into(),
                    title: "활성 프로파일".into(),
                    default: json!(""),
                },
                SettingRegistration {
                    key: "spring.env".into(),
                    title: "환경변수".into(),
                    default: json!({}),
                },
            ],
            ..Default::default()
        },
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpringBuild {
    Maven,
    Gradle,
}
pub fn detect_spring_builds(root: &Path) -> Vec<SpringBuild> {
    let mut found = Vec::new();
    if root.join("pom.xml").is_file() {
        found.push(SpringBuild::Maven);
    }
    if ["build.gradle", "build.gradle.kts"]
        .iter()
        .any(|p| root.join(p).is_file())
    {
        found.push(SpringBuild::Gradle);
    }
    found
}
fn spring_response(request: &ExtensionRequest) -> Result<Vec<TaskProfile>> {
    ensure!(
        request.command == "profiles",
        "지원하지 않는 Spring 명령입니다"
    );
    let settings = &request.settings;
    let string = |key: &str| {
        settings
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let mut env: BTreeMap<String, String> = settings
        .get("spring.env")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let jdk = string("spring.jdk");
    if !jdk.is_empty() {
        let home = PathBuf::from(&jdk)
            .canonicalize()
            .context("JDK 홈을 찾을 수 없습니다")?;
        ensure!(
            home.join("bin/java").is_file(),
            "JDK 홈에 bin/java가 없습니다"
        );
        env.insert("JAVA_HOME".into(), home.display().to_string());
        let path = std::env::var("PATH").unwrap_or_default();
        env.insert("PATH".into(), format!("{}/bin:{path}", home.display()));
    }
    let args: Vec<String> = settings
        .get("spring.args")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let profiles = string("spring.profiles");
    let mut tasks = Vec::new();
    for build in detect_spring_builds(&request.worktree) {
        let (name, program, mut command) = match build {
            SpringBuild::Maven => {
                let configured = string("spring.maven");
                let program = if !configured.is_empty() {
                    configured
                } else if request.worktree.join("mvnw").is_file() {
                    request.worktree.join("mvnw").display().to_string()
                } else {
                    "mvn".into()
                };
                let mut command = vec!["spring-boot:run".into()];
                if !profiles.is_empty() {
                    command.push(format!("-Dspring-boot.run.profiles={profiles}"));
                }
                ("Spring · Maven", program, command)
            }
            SpringBuild::Gradle => {
                let configured = string("spring.gradle");
                let program = if !configured.is_empty() {
                    configured
                } else if request.worktree.join("gradlew").is_file() {
                    request.worktree.join("gradlew").display().to_string()
                } else {
                    "gradle".into()
                };
                let mut command = vec!["bootRun".into()];
                if !profiles.is_empty() {
                    command.push(format!("--args=--spring.profiles.active={profiles}"));
                }
                ("Spring · Gradle", program, command)
            }
        };
        command.extend(args.clone());
        command.insert(0, program);
        tasks.push(TaskProfile {
            name: name.into(),
            command,
            cwd: PathBuf::new(),
            env: env.clone(),
        });
    }
    ensure!(
        !tasks.is_empty(),
        "pom.xml 또는 build.gradle(.kts)을 찾을 수 없습니다"
    );
    Ok(tasks)
}
/// Runs before GPUI initialization, in its own owned extension process.
pub fn run_builtin_spring() -> i32 {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut out = std::io::stdout().lock();
    loop {
        let mut bytes = Vec::new();
        let count = (&mut reader)
            .take(256 * 1024 + 1)
            .read_until(b'\n', &mut bytes);
        match count {
            Ok(0) => return 0,
            Ok(n) if n <= 256 * 1024 => {}
            _ => return 2,
        };
        let request: ExtensionRequest = match serde_json::from_slice(&bytes) {
            Ok(r) => r,
            Err(_) => return 2,
        };
        let result = if request.api_version == HOST_API_VERSION {
            spring_response(&request)
        } else {
            Err(anyhow::anyhow!("호환되지 않는 API"))
        };
        let (tasks, error) = match result {
            Ok(t) => (t, None),
            Err(e) => (vec![], Some(e.to_string())),
        };
        let response = ExtensionResponse {
            api_version: HOST_API_VERSION.into(),
            request_id: request.request_id,
            generation: request.generation,
            worktree: request.worktree,
            tasks,
            logs: vec![
                "Spring 메타데이터를 읽었습니다. 빌드와 실행은 작업에서 직접 선택하세요.".into(),
            ],
            error,
        };
        if serde_json::to_writer(&mut out, &response).is_err()
            || out.write_all(b"\n").is_err()
            || out.flush().is_err()
        {
            return 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn fixture(dir: &Path, version: &str) -> ExtensionManifest {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("plugin.py"),r##"#!/usr/bin/python3
import sys,json,os
for line in sys.stdin:
 r=json.loads(line)
 print('external plugin stderr',file=sys.stderr,flush=True)
 print(json.dumps({'api_version':'1.0.42','request_id':r['request_id'],'generation':r['generation']+(1 if r['settings'].get('stale') else 0),'worktree':r['worktree'],'tasks':[{'name':'External fixture','command':['/bin/echo','literal $(unchanged)'],'cwd':'','env':{'FIXTURE':'1'}}],'logs':['plugin pid '+str(os.getpid())+' '+str(r['settings'].get('greeting',''))],'error':None}),flush=True)
"##).unwrap();
        fs::set_permissions(dir.join("plugin.py"), fs::Permissions::from_mode(0o755)).unwrap();
        let manifest = ExtensionManifest {
            schema_version: 1,
            api_version: "1.0.99".into(),
            id: "fixture.external".into(),
            name: "External fixture".into(),
            version: version.into(),
            executable: "plugin.py".into(),
            args: vec![],
            capabilities: CapabilityRegistration {
                commands: vec![CommandRegistration {
                    id: "generate".into(),
                    title: "Generate fixture".into(),
                }],
                settings: vec![SettingRegistration {
                    key: "greeting".into(),
                    title: "Greeting".into(),
                    default: json!("hello"),
                }],
                language_servers: vec![ToolRegistration {
                    extension_id: "".into(),
                    language: "fixture".into(),
                    executable: "plugin.py".into(),
                    args: vec!["--lsp".into()],
                    file_extensions: vec!["fixture".into()],
                }],
                formatters: vec![ToolRegistration {
                    extension_id: "".into(),
                    language: "fixture".into(),
                    executable: "plugin.py".into(),
                    args: vec!["--format".into()],
                    file_extensions: vec!["fixture".into()],
                }],
            },
        };
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        manifest
    }
    #[test]
    fn local_lifecycle_and_external_protocol() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("bundle");
        fixture(&source, "1.0.0");
        let mut registry = ExtensionRegistry::new(temp.path().join("installed"));
        let id = registry.install(&source, false).unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let host = ExtensionHost::default();
        let extension = registry.get(&id).unwrap();
        assert!(!extension.enabled);
        assert!(!host.active(&root, &id));
        assert!(registry.language_servers(&HashMap::new()).is_empty());
        assert!(
            host.invoke(
                &extension,
                &root,
                "generate",
                json!({}),
                true,
                true,
                LogOptions::default()
            )
            .is_err()
        );
        registry.set_enabled(&id, true).unwrap();
        let extension = registry.get(&id).unwrap();
        let tools = registry.language_servers(&HashMap::new());
        assert_eq!(tools.len(), 1);
        assert!(
            tools[0]
                .executable
                .starts_with(registry.directory.canonicalize().unwrap())
        );
        assert_eq!(registry.formatters(&HashMap::new()).len(), 1);
        assert!(!host.active(&root, &id));
        assert!(
            host.invoke(
                &extension,
                &root,
                "generate",
                json!({}),
                false,
                true,
                LogOptions::default()
            )
            .is_err()
        );
        assert!(
            host.invoke(
                &extension,
                &root,
                "generate",
                json!({}),
                true,
                false,
                LogOptions::default()
            )
            .is_err()
        );
        let response = host
            .invoke(
                &extension,
                &root,
                "generate",
                json!({"greeting":"scoped"}),
                true,
                true,
                LogOptions::default(),
            )
            .unwrap();
        assert_eq!(response.tasks[0].command[1], "literal $(unchanged)");
        assert!(response.logs[0].contains("scoped"));
        assert!(host.active(&root, &id));
        let sibling = temp.path().join("sibling");
        fs::create_dir(&sibling).unwrap();
        let sibling = sibling.canonicalize().unwrap();
        let other = host
            .invoke(
                &extension,
                &sibling,
                "generate",
                json!({"greeting":"other"}),
                true,
                true,
                LogOptions::default(),
            )
            .unwrap();
        assert_ne!(other.logs[0], response.logs[0]);
        assert!(
            host.invoke(
                &extension,
                &root,
                "generate",
                json!({"stale":true}),
                true,
                true,
                LogOptions::default()
            )
            .is_err()
        );
        assert!(!host.active(&root, &id));
        assert!(host.active(&sibling, &id));
        host.deactivate_extension(&id);
        assert!(!host.active(&sibling, &id));
        fixture(&source, "1.1.0");
        registry.install(&source, true).unwrap();
        assert_eq!(registry.get(&id).unwrap().manifest.version, "1.1.0");
        assert!(registry.install(&source, true).is_err());
        registry.remove(&id).unwrap();
        assert!(registry.get(&id).is_err());
    }
    #[test]
    fn manifests_reject_versions_paths_links_and_reserved_ids() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("bundle");
        let mut m = fixture(&bundle, "1.0.0");
        m.api_version = "2.0.0".into();
        assert!(m.validate(&bundle).is_err());
        m.api_version = "1.1.0".into();
        assert!(m.validate(&bundle).is_err());
        m.api_version = "1.0.2".into();
        m.executable = "../escape".into();
        assert!(m.validate(&bundle).is_err());
        m.executable = "plugin.py".into();
        m.id = SPRING_ID.into();
        assert!(m.validate(&bundle).is_err());
        std::os::unix::fs::symlink("/etc/passwd", bundle.join("escape")).unwrap();
        let mut registry = ExtensionRegistry::new(temp.path().join("installed"));
        assert!(registry.install(&bundle, false).is_err());
        assert_eq!(registry.list().len(), 1);
    }
    #[test]
    fn spring_metadata_commands_are_argv_and_never_executed() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("pom.xml"), "<project/>").unwrap();
        fs::write(temp.path().join("build.gradle.kts"), "plugins {}").unwrap();
        let jdk = temp.path().join("jdk");
        fs::create_dir_all(jdk.join("bin")).unwrap();
        fs::write(jdk.join("bin/java"), "fixture").unwrap();
        let request = ExtensionRequest {
            api_version: HOST_API_VERSION.into(),
            request_id: 2,
            generation: 3,
            worktree: temp.path().canonicalize().unwrap(),
            command: "profiles".into(),
            settings: json!({"spring.jdk":jdk,"spring.maven":"/configured/maven","spring.gradle":"/configured/gradle","spring.args":["--info","literal $(not-run)"],"spring.profiles":"local,dev","spring.env":{"CUSTOM":"yes"}}),
        };
        let tasks = spring_response(&request).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].command[0], "/configured/maven");
        assert!(
            tasks[0]
                .command
                .contains(&"-Dspring-boot.run.profiles=local,dev".into())
        );
        assert!(
            tasks[1]
                .command
                .contains(&"--args=--spring.profiles.active=local,dev".into())
        );
        assert_eq!(tasks[0].env["CUSTOM"], "yes");
        assert!(tasks[0].env["JAVA_HOME"].ends_with("jdk"));
        assert!(tasks[1].command.contains(&"literal $(not-run)".into()));
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, time::Instant};
    #[test]
    fn deactivation_cancels_a_plugin_which_never_reads_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let bundle = root.join("bundle");
        fs::create_dir(&bundle).unwrap();
        fs::write(bundle.join("blocked"), "#!/bin/sh\nexec /bin/sleep 20\n").unwrap();
        fs::set_permissions(bundle.join("blocked"), fs::Permissions::from_mode(0o755)).unwrap();
        let extension = InstalledExtension {
            manifest: ExtensionManifest {
                schema_version: 1,
                api_version: HOST_API_VERSION.into(),
                id: "fixture.blocked".into(),
                name: "blocked".into(),
                version: "1.0.0".into(),
                executable: "blocked".into(),
                args: vec![],
                capabilities: CapabilityRegistration {
                    commands: vec![CommandRegistration {
                        id: "blocked".into(),
                        title: "blocked".into(),
                    }],
                    ..Default::default()
                },
            },
            bundle,
            enabled: true,
            builtin: false,
        };
        let host = Arc::new(ExtensionHost::default());
        let worker = host.clone();
        let worktree = root.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = worker.invoke(
                &extension,
                &worktree,
                "blocked",
                json!({"large":"x".repeat(200_000)}),
                true,
                true,
                LogOptions::default(),
            );
            tx.send(result.is_err()).unwrap();
        });
        let until = Instant::now() + Duration::from_secs(3);
        while !host.active(&root, "fixture.blocked") {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
        host.deactivate(&root, "fixture.blocked");
        assert!(rx.recv_timeout(Duration::from_secs(3)).unwrap());
        assert!(!host.active(&root, "fixture.blocked"));
    }
}
