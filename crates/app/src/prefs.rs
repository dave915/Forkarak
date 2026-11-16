use pokarak_core::CompareMode;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub recent: Vec<PathBuf>,
    pub last_root: Option<PathBuf>,
    pub worktrees: HashMap<PathBuf, SavedView>,
    pub trusted: Vec<PathBuf>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedView {
    pub selected: Option<PathBuf>,
    pub tabs: Vec<PathBuf>,
    pub code: bool,
    pub all_files: bool,
    pub comparison: CompareMode,
    pub diff_row: usize,
    pub line: u32,
    pub column: u32,
    pub scroll_x: f32,
    pub scroll_y: f32,
}

pub fn config_path() -> PathBuf {
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
