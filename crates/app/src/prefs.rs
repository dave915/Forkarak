use pokarak_core::CompareMode;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
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
    pub theme: crate::theme::ThemeChoice,
    #[serde(flatten)]
    pub display: crate::theme::DisplaySettings,
    pub split_diff: bool,
    pub show_branches: bool,
    pub dismissed_update: Option<String>,
    pub recent: Vec<PathBuf>,
    pub last_root: Option<PathBuf>,
    pub worktrees: HashMap<PathBuf, SavedView>,
    pub trusted: Vec<PathBuf>,
    pub open_projects: Vec<SavedProject>,
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
    pub graph_detail_collapsed: bool,
    pub graph_branch: Option<String>,
    pub collapsed_graph_sections: HashSet<String>,
    pub sidebar_width: Option<f32>,
    pub code_files_width: Option<f32>,
    pub graph_height: Option<f32>,
    pub graph_files_width: Option<f32>,
    pub diff_split_ratio: Option<f32>,
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
    pub fn load() -> Self {
        std::fs::read(config_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let temp = path.with_extension("tmp");
            if std::fs::write(&temp, bytes).is_ok() {
                let _ = std::fs::rename(temp, path);
            }
        }
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
