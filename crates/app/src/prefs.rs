use pokarak_core::CompareMode;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

static SESSION_DIR: OnceLock<PathBuf> = OnceLock::new();
pub fn set_session_dir(path: PathBuf) {
    let _ = SESSION_DIR.set(path);
}

pub fn explicit_session_dir() -> Option<&'static std::path::Path> {
    SESSION_DIR.get().map(PathBuf::as_path)
}

#[derive(Default, Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
pub struct UpdateWorkspace {
    pub root: Option<PathBuf>,
    pub view: UpdateView,
    pub messages: HashMap<PathBuf, String>,
}

#[derive(Serialize, Deserialize)]
pub enum UpdateView {
    Graph(Option<SavedCommit>),
    Code,
    Changes,
}

#[derive(Serialize, Deserialize)]
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
impl Preferences {
    pub fn option_bool(&self, root: Option<&Path>, key: &str, fallback: bool) -> bool {
        self.option(root, key).and_then(serde_json::Value::as_bool).unwrap_or(fallback)
    }

    pub fn option_usize(&self, root: Option<&Path>, key: &str, fallback: usize) -> usize {
        self.option(root, key).and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok()).unwrap_or(fallback)
    }

    pub fn option_string(&self, root: Option<&Path>, key: &str, fallback: &str) -> String {
        self.option(root, key).and_then(serde_json::Value::as_str).unwrap_or(fallback).to_owned()
    }

    pub fn option(&self, root: Option<&Path>, key: &str) -> Option<&serde_json::Value> {
        root.and_then(|root| self.project_options.get(root)).and_then(|options| options.get(key))
            .or_else(|| self.options.get(key))
    }

    pub fn set_option(&mut self, root: Option<&Path>, key: &str, value: serde_json::Value) {
        match root {
            Some(root) => self.project_options.entry(root.to_path_buf()).or_default(),
            None => &mut self.options,
        }.insert(key.to_owned(), value);
    }

    pub fn remove_option(&mut self, root: Option<&Path>, key: &str) {
        if let Some(options) = root.map(|root| self.project_options.get_mut(root)).unwrap_or(Some(&mut self.options)) {
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
        let parent = path.parent().expect("session configuration directory");
        std::fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(temp.as_file_mut(), self)?;
        temp.persist(path)?;
        Ok(())
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
