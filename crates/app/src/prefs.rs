use crate::theme::DisplaySettings;
use pokarak_core::CompareMode;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

pub const KEYBOARD_OPTIONS: [&str; 2] = ["keyboard.preset", "keyboard.bindings"];

static SESSION_DIR: OnceLock<PathBuf> = OnceLock::new();
pub fn set_session_dir(path: PathBuf) {
    let _ = SESSION_DIR.set(path);
}

pub fn explicit_session_dir() -> Option<&'static std::path::Path> {
    SESSION_DIR.get().map(PathBuf::as_path)
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Application defaults. Unknown keys are retained for forward compatibility.
    pub options: HashMap<String, serde_json::Value>,
    /// Project overrides, stored outside inspected repositories.
    pub project_options: HashMap<PathBuf, HashMap<String, serde_json::Value>>,
    pub theme: crate::theme::ThemeChoice,
    #[serde(flatten)]
    pub display: crate::theme::DisplaySettings,
    pub split_diff: bool,
    pub show_branches: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_workspace: Option<UpdateWorkspace>,
    pub dismissed_update: Option<String>,
    pub recent: Vec<PathBuf>,
    pub last_root: Option<PathBuf>,
    pub worktrees: HashMap<PathBuf, SavedView>,
    pub trusted: Vec<PathBuf>,
    pub open_projects: Vec<SavedProject>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UpdateWorkspace {
    pub root: Option<PathBuf>,
    pub view: UpdateView,
    pub messages: HashMap<PathBuf, String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum UpdateView {
    Graph(Option<SavedCommit>),
    Code,
    Changes,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedCommit {
    pub oid: String,
    pub parent: usize,
    pub file: Option<PathBuf>,
    pub code: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedProject {
    pub root: PathBuf,
    pub last_worktree: PathBuf,
    pub worktrees: Vec<PathBuf>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedPosition {
    pub diff_row: usize,
    pub line: u32,
    pub column: u32,
    pub scroll_x: f32,
    pub scroll_y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphColumn {
    Refs,
    Graph,
    Message,
    Author,
    Date,
    Hash,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedView {
    pub selected: Option<PathBuf>,
    pub tabs: Vec<PathBuf>,
    pub code: bool,
    pub all_files: bool,
    pub hide_sidebar: bool,
    /// Git sidebar visibility in Code view. `None` (views saved before this
    /// field existed) shares `hide_sidebar` with Git view, as it always did.
    /// New worktrees start with `Some(false)`: the file tree's worktree
    /// picker covers switching, and the code gets the width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_sidebar: Option<bool>,
    pub hide_files: bool,
    pub collapsed_folders: HashSet<PathBuf>,
    pub graph_detail_collapsed: bool,
    pub graph_branch: Option<String>,
    pub collapsed_graph_sections: HashSet<String>,
    pub sidebar_width: Option<f32>,
    pub code_files_width: Option<f32>,
    pub graph_height: Option<f32>,
    pub graph_files_width: Option<f32>,
    pub diff_split_ratio: Option<f32>,
    pub diff_hide_whitespace: bool,
    pub graph_columns: HashMap<GraphColumn, f32>,
    pub filter: String,
    pub text_search: bool,
    pub comparison: CompareMode,
    pub diff_row: usize,
    pub line: u32,
    pub column: u32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub positions: HashMap<PathBuf, SavedPosition>,
}

pub fn config_path() -> PathBuf {
    if let Some(path) = explicit_session_dir() {
        return path.join("session.json");
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config/pokarak/session.json")
}

// The session is atomically replaced, so all processes lock the same sibling file.
fn lock_session(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(path.parent().expect("session configuration directory"))?;
    let lock = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    Ok(lock)
}

// A --new-window launch has its own process. Read only the shared settings
// from the session when another window may have updated them.
#[derive(Default, Deserialize)]
struct SharedSettings {
    #[serde(flatten)]
    display: DisplaySettings,
    #[serde(default)]
    options: KeyboardOptions,
    #[serde(default)]
    project_options: HashMap<PathBuf, KeyboardOptions>,
}

#[derive(Default, Deserialize)]
struct KeyboardOptions {
    #[serde(rename = "keyboard.preset")]
    preset: Option<serde_json::Value>,
    #[serde(rename = "keyboard.bindings")]
    bindings: Option<serde_json::Value>,
}

impl KeyboardOptions {
    fn apply(self, map: &mut HashMap<String, serde_json::Value>) {
        for (key, value) in KEYBOARD_OPTIONS
            .into_iter()
            .zip([self.preset, self.bindings])
        {
            if let Some(value) = value {
                map.insert(key.into(), value);
            } else {
                map.remove(key);
            }
        }
    }
}

impl SharedSettings {
    fn read(path: &Path) -> std::io::Result<Option<Self>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn apply_keyboard(mut self, prefs: &mut Preferences) {
        self.options.apply(&mut prefs.options);
        for (root, map) in &mut prefs.project_options {
            self.project_options
                .remove(root)
                .unwrap_or_default()
                .apply(map);
        }
        for (root, keys) in self.project_options {
            if keys.preset.is_some() || keys.bindings.is_some() {
                keys.apply(prefs.project_options.entry(root).or_default());
            }
        }
    }
}

impl Preferences {
    /// Application-only: project overrides must never change the app's Dock icon.
    pub fn app_icon(&self) -> crate::app_icon::IconChoice {
        self.options
            .get(crate::app_icon::PREFERENCE_KEY)
            .and_then(serde_json::Value::as_str)
            .and_then(crate::app_icon::IconChoice::from_id)
            .unwrap_or_default()
    }

    pub fn set_app_icon(&mut self, choice: crate::app_icon::IconChoice) {
        self.set_option(None, crate::app_icon::PREFERENCE_KEY, choice.id().into());
    }

    pub fn option_bool(&self, root: Option<&Path>, key: &str, fallback: bool) -> bool {
        self.option(root, key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(fallback)
    }

    pub fn option_usize(&self, root: Option<&Path>, key: &str, fallback: usize) -> usize {
        self.option(root, key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(fallback)
    }

    pub fn option_string(&self, root: Option<&Path>, key: &str, fallback: &str) -> String {
        self.option(root, key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or(fallback)
            .to_owned()
    }

    pub fn option(&self, root: Option<&Path>, key: &str) -> Option<&serde_json::Value> {
        self.scoped_option(root, key)
            .or_else(|| self.options.get(key))
    }

    pub fn scoped_option(&self, root: Option<&Path>, key: &str) -> Option<&serde_json::Value> {
        match root {
            Some(root) => self.project_options.get(root)?.get(key),
            None => self.options.get(key),
        }
    }

    pub fn set_option(&mut self, root: Option<&Path>, key: &str, value: serde_json::Value) {
        match root {
            Some(root) => self.project_options.entry(root.to_path_buf()).or_default(),
            None => &mut self.options,
        }
        .insert(key.to_owned(), value);
    }

    pub fn remove_option(&mut self, root: Option<&Path>, key: &str) {
        if let Some(options) = root
            .map(|root| self.project_options.get_mut(root))
            .unwrap_or(Some(&mut self.options))
        {
            options.remove(key);
        }
    }

    pub fn load() -> Self {
        std::fs::read(config_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        let _ = self.try_save();
    }

    pub fn try_save(&self) -> std::io::Result<()> {
        let path = config_path();
        self.try_save_to(&path)
    }

    fn try_save_to(&self, path: &Path) -> std::io::Result<()> {
        self.save_workspace(path, false)
    }

    fn save_workspace(&self, path: &Path, reset_display: bool) -> std::io::Result<()> {
        let _lock = lock_session(path)?;
        if let Some(shared) = SharedSettings::read(path)? {
            let mut snapshot = self.clone();
            if !reset_display {
                snapshot.display = shared.display.validated();
            }
            shared.apply_keyboard(&mut snapshot);
            snapshot.write_session(path)
        } else {
            self.write_session(path)
        }
    }

    pub fn refresh_shared_settings(&mut self) -> Option<DisplaySettings> {
        let latest = SharedSettings::read(&config_path()).ok()??;
        let display = latest.display.validated();
        latest.apply_keyboard(self);
        Some(display)
    }

    pub fn try_save_appearance_reset(&self) -> std::io::Result<()> {
        self.save_workspace(&config_path(), true)
    }

    pub fn save_keyboard(
        &mut self,
        root: Option<&Path>,
        edits: &[(&str, Option<serde_json::Value>)],
    ) -> std::io::Result<()> {
        self.save_keyboard_at(&config_path(), root, edits)
    }

    fn save_keyboard_at(
        &mut self,
        path: &Path,
        root: Option<&Path>,
        edits: &[(&str, Option<serde_json::Value>)],
    ) -> std::io::Result<()> {
        let _lock = lock_session(path)?;
        let mut latest: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.clone(),
            Err(error) => return Err(error),
        };
        for &(key, ref value) in edits {
            if key == "keyboard.bindings"
                && let Some(edited) = value.as_ref().and_then(|v| v.as_object())
            {
                // Carry only the edits made to this scope into the latest saved map.
                let previous = self.scoped_option(root, key).and_then(|v| v.as_object());
                let mut merged = latest
                    .scoped_option(root, key)
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                if let Some(previous) = previous {
                    merged.retain(|id, _| !previous.contains_key(id) || edited.contains_key(id));
                }
                for (id, binding) in edited {
                    if previous.and_then(|p| p.get(id)) != Some(binding) {
                        merged.insert(id.clone(), binding.clone());
                    }
                }
                latest.set_option(root, key, merged.into());
            } else if let Some(value) = value {
                latest.set_option(root, key, value.clone());
            } else {
                latest.remove_option(root, key);
            }
        }
        latest.write_session(path)?;
        // Synchronize this window under the same lock, without adopting another
        // window's selected project, open tabs or display state.
        if let Some(shared) = SharedSettings::read(path)? {
            shared.apply_keyboard(self);
        }
        Ok(())
    }

    pub fn update_display(
        step: impl FnOnce(DisplaySettings) -> DisplaySettings,
    ) -> std::io::Result<DisplaySettings> {
        Self::update_display_at(&config_path(), step)
    }

    fn update_display_at(
        path: &Path,
        step: impl FnOnce(DisplaySettings) -> DisplaySettings,
    ) -> std::io::Result<DisplaySettings> {
        let _lock = lock_session(path)?;
        let mut latest: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e),
        };
        let display = step(latest.display.validated());
        if display != latest.display {
            latest.display = display;
            latest.write_session(path)?;
        }
        Ok(display)
    }

    fn write_session(&self, path: &Path) -> std::io::Result<()> {
        let parent = path.parent().expect("session configuration directory");
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(temp.as_file_mut(), self)?;
        temp.persist(path)?;
        Ok(())
    }
}

#[cfg(test)]
mod app_icon_tests {
    use super::Preferences;
    use crate::app_icon::{IconChoice, PREFERENCE_KEY};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn reading_default_and_saved_colors_never_rewrites_the_selection() {
        for value in [None, Some("future-icon"), Some("follow_default")]
            .into_iter()
            .chain(IconChoice::ALL.into_iter().map(|choice| Some(choice.id())))
        {
            let mut prefs = Preferences::default();
            if let Some(value) = value {
                prefs.options.insert(PREFERENCE_KEY.into(), json!(value));
            }
            let before = prefs.options.clone();
            let expected = value.and_then(IconChoice::from_id).unwrap_or_default();
            assert_eq!(prefs.app_icon(), expected);
            assert_eq!(prefs.options, before);
            let next: Preferences =
                serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
            assert_eq!(next.app_icon(), expected);
            assert_eq!(next.options, before);
        }
    }

    #[test]
    fn explicit_colors_and_follow_default_survive_real_session_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.json");
        let mut prefs: Preferences = serde_json::from_value(json!({
            "code_font_size": 19,
            "split_diff": true,
            "options": { "future.setting": { "preserved": true } }
        }))
        .unwrap();
        assert_eq!(prefs.app_icon(), IconChoice::FollowDefault);
        for choice in IconChoice::ALL {
            prefs.set_app_icon(choice);
            prefs.try_save_to(&path).unwrap();
            prefs = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(prefs.app_icon(), choice);
            assert_eq!(prefs.options[PREFERENCE_KEY], choice.id());
            assert_eq!(prefs.display.code_font_size, 19.);
            assert!(prefs.split_diff);
            assert_eq!(prefs.options["future.setting"]["preserved"], true);
        }
        prefs.set_app_icon(IconChoice::Charcoal);
        assert!(prefs.app_icon().override_bytes().is_some());
        prefs.set_app_icon(IconChoice::FollowDefault);
        prefs.try_save_to(&path).unwrap();
        let next: Preferences = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        // Clearing the runtime override delegates to the next installed bundle,
        // rather than persisting the current bundle's charcoal image as a color.
        assert!(next.app_icon().override_bytes().is_none());
        assert_eq!(next.options[PREFERENCE_KEY], "follow_default");
    }

    #[test]
    fn project_and_unknown_icon_values_cannot_reset_other_preferences() {
        let mut prefs = Preferences::default();
        prefs.set_app_icon(IconChoice::Ivory);
        prefs.set_option(
            Some(Path::new("/fixture/project")),
            PREFERENCE_KEY,
            json!("deep-blue"),
        );
        assert_eq!(prefs.app_icon(), IconChoice::Ivory);
        prefs.set_option(None, PREFERENCE_KEY, json!("future-icon"));
        prefs.options.insert("preserve.me".into(), json!(42));
        let next: Preferences =
            serde_json::from_value(serde_json::to_value(&prefs).unwrap()).unwrap();
        assert_eq!(next.app_icon(), IconChoice::FollowDefault);
        assert_eq!(next.options["preserve.me"], 42);
        prefs.set_app_icon(IconChoice::Charcoal);
        prefs.remove_option(None, PREFERENCE_KEY);
        assert_eq!(prefs.app_icon(), IconChoice::FollowDefault);
    }
}

#[cfg(test)]
mod display_tests {
    use super::Preferences;
    use crate::theme::DisplaySettings;

    #[test]
    fn legacy_session_keeps_code_font_and_defaults_new_preferences() {
        let old: Preferences =
            serde_json::from_str(r#"{"code_font_size":19,"split_diff":true}"#).unwrap();
        let saved = serde_json::to_value(&old).unwrap();
        assert_eq!(old.display.code_font_size, 19.);
        assert_eq!(old.display.ui_font_size, 13.);
        assert_eq!(saved["ui_zoom_percent"], 100);
        assert_eq!(saved["code_font_size"], 19.);
        assert!(old.split_diff);
    }

    #[test]
    fn a_stale_workspace_save_preserves_zoom_and_concurrent_steps_accumulate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let mut stale_window = Preferences::default();
        stale_window.try_save_to(&path).unwrap();

        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    Preferences::update_display_at(&path, |mut display| {
                        display.ui_zoom_percent += 5;
                        display
                    })
                    .unwrap();
                });
            }
        });
        stale_window.split_diff = true;
        stale_window.try_save_to(&path).unwrap();
        let saved: Preferences = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.display.ui_zoom_percent, 110);
        assert!(saved.split_diff);
        let display = Preferences::update_display_at(&path, |mut display| {
            display.ui_font_size = 15.;
            display
        })
        .unwrap();
        assert_eq!(display.ui_zoom_percent, 110);
        let saved: Preferences = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(saved.split_diff);
        assert_eq!(saved.display, display);
    }

    #[test]
    fn separate_windows_merge_keyboard_edits_and_workspace_saves_cannot_restore_old_keys() {
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let project = dir.path().join("project");
        let mut first = Preferences::default();
        first.set_option(None, "keyboard.bindings", json!({"refresh":"f8"}));
        first.try_save_to(&path).unwrap();
        let mut second = first.clone();
        first
            .save_keyboard_at(
                &path,
                None,
                &[
                    ("keyboard.preset", Some(json!("intellij"))),
                    (
                        "keyboard.bindings",
                        Some(json!({"refresh":"f8","find_file":"f7"})),
                    ),
                ],
            )
            .unwrap();
        second
            .save_keyboard_at(
                &path,
                None,
                &[("keyboard.bindings", Some(json!({"refresh":""})))],
            )
            .unwrap();
        second
            .save_keyboard_at(
                &path,
                Some(&project),
                &[("keyboard.preset", Some(json!("vscode")))],
            )
            .unwrap();

        first.try_save_to(&path).unwrap();
        let read =
            || serde_json::from_slice::<Preferences>(&std::fs::read(&path).unwrap()).unwrap();
        let persisted = read();
        assert_eq!(
            persisted.option(None, "keyboard.bindings"),
            Some(&json!({"refresh":"","find_file":"f7"}))
        );
        assert_eq!(
            persisted.option(None, "keyboard.preset"),
            Some(&json!("intellij"))
        );
        assert_eq!(
            persisted.option(Some(&project), "keyboard.preset"),
            Some(&json!("vscode"))
        );

        // The activation path only refreshes shared keyboard settings.
        first.last_root = Some(project.clone());
        super::SharedSettings::read(&path)
            .unwrap()
            .unwrap()
            .apply_keyboard(&mut first);
        assert_eq!(first.last_root.as_ref(), Some(&project));
        assert_eq!(
            first.option(None, "keyboard.bindings"),
            persisted.option(None, "keyboard.bindings")
        );
        first
            .save_keyboard_at(&path, Some(&project), &[("keyboard.preset", None)])
            .unwrap();
        second.save_workspace(&path, true).unwrap();
        assert_eq!(
            read().option(Some(&project), "keyboard.preset"),
            Some(&json!("intellij"))
        );
    }

    #[test]
    fn invalid_display_values_fall_back_independently() {
        let input = DisplaySettings {
            ui_font_size: f32::NAN,
            code_font_size: 20.,
            ui_zoom_percent: 0,
        };
        assert_eq!(
            input.validated(),
            DisplaySettings {
                code_font_size: 20.,
                ..Default::default()
            }
        );
    }
}

#[cfg(test)]
mod pane_visibility_tests {
    use super::{Preferences, SavedView};
    use std::path::Path;

    #[test]
    fn legacy_and_new_worktrees_default_to_visible_panels() {
        let prefs: Preferences = serde_json::from_str(
            r#"{"worktrees":{"/project":{"selected":"src/main.rs","sidebar_width":247,"code_files_width":311}}}"#,
        ).unwrap();
        let legacy = &prefs.worktrees[Path::new("/project")];
        assert!(!legacy.hide_sidebar && !legacy.hide_files);
        assert_eq!(legacy.sidebar_width, Some(247.));
        assert_eq!(legacy.code_files_width, Some(311.));
        let fresh = SavedView::default();
        assert!(!fresh.hide_sidebar && !fresh.hide_files);
    }

    #[test]
    fn panel_visibility_round_trips_independently_for_each_worktree() {
        let mut prefs = Preferences::default();
        prefs.worktrees.insert(
            "/project".into(),
            SavedView {
                hide_sidebar: true,
                sidebar_width: Some(245.),
                ..Default::default()
            },
        );
        prefs.worktrees.insert(
            "/linked-worktree".into(),
            SavedView {
                hide_files: true,
                code_files_width: Some(315.),
                ..Default::default()
            },
        );
        let restored: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        let project = &restored.worktrees[Path::new("/project")];
        let linked = &restored.worktrees[Path::new("/linked-worktree")];
        assert!(project.hide_sidebar && !project.hide_files);
        assert!(!linked.hide_sidebar && linked.hide_files);
        assert_eq!(project.sidebar_width, Some(245.));
        assert_eq!(linked.code_files_width, Some(315.));
    }
}

#[cfg(test)]
mod option_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn legacy_unknown_and_project_options_round_trip_without_scope_leaks() {
        let mut prefs: Preferences = serde_json::from_str(r#"{"split_diff":true}"#).unwrap();
        assert!(prefs.options.is_empty() && prefs.project_options.is_empty());
        let first = Path::new("/authored/first");
        let second = Path::new("/authored/second");
        prefs.set_option(None, "editor.tab_size", json!(4));
        prefs.set_option(None, "future.setting", json!({"keep":[1,2,3]}));
        prefs.set_option(Some(first), "editor.tab_size", json!(2));
        prefs.set_option(Some(first), "editor.direct_edit", json!(true));
        let mut restored: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert_eq!(restored.option_usize(Some(first), "editor.tab_size", 8), 2);
        assert_eq!(restored.option_usize(Some(second), "editor.tab_size", 8), 4);
        assert!(restored.option_bool(Some(first), "editor.direct_edit", false));
        assert!(!restored.option_bool(Some(second), "editor.direct_edit", false));
        restored.remove_option(Some(first), "editor.tab_size");
        assert_eq!(restored.option_usize(Some(first), "editor.tab_size", 8), 4);
        assert_eq!(
            restored.option(None, "future.setting"),
            Some(&json!({"keep":[1,2,3]}))
        );
        assert!(restored.split_diff);
    }
    #[test]
    fn malformed_typed_values_use_callers_fallbacks() {
        let mut prefs = Preferences::default();
        let root = Path::new("/authored");
        prefs.set_option(None, "number", json!(4));
        prefs.set_option(Some(root), "number", json!(-1));
        prefs.set_option(None, "bool", json!("true"));
        prefs.set_option(None, "text", json!(false));
        assert_eq!(prefs.option_usize(Some(root), "number", 8), 8);
        assert!(prefs.option_bool(None, "bool", true));
        assert_eq!(prefs.option_string(None, "text", "fallback"), "fallback");
        prefs.remove_option(Some(Path::new("/missing")), "number");
        assert_eq!(prefs.option_usize(None, "number", 0), 4);
    }
}
