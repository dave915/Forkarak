//! Search option identity is project-scoped, while results remain worktree-scoped.
use super::*;
#[derive(Clone)]
pub(super) struct Exclusions {
    project: Option<PathBuf>,
    raw: String,
}
impl Exclusions {
    pub fn read(prefs: &Preferences, project: Option<&Path>) -> Self {
        Self {
            project: project.map(Path::to_path_buf),
            raw: prefs.option_string(project, "search.exclude", ""),
        }
    }
    pub fn patterns(&self) -> Vec<String> {
        self.raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect()
    }
    pub fn is_current(&self, prefs: &Preferences, project: Option<&Path>) -> bool {
        self.project.as_deref() == project
            && self.raw == prefs.option_string(project, "search.exclude", "")
    }
}
#[cfg(test)]
mod tests {
    use super::Exclusions;
    use crate::prefs::Preferences;
    use std::path::Path;
    #[test]
    fn project_exclusions_preserve_rule_spelling_and_reject_late_preferences() {
        let mut prefs = Preferences::default();
        let project = Path::new("/fixture/project");
        prefs.set_option(None, "search.exclude", serde_json::json!("vendor/"));
        prefs.set_option(
            Some(project),
            "search.exclude",
            serde_json::json!("\n**/generated/**\n!src/keep.ts\n\\#literal.ts\n"),
        );
        let captured = Exclusions::read(&prefs, Some(project));
        assert_eq!(
            captured.patterns(),
            vec!["**/generated/**", "!src/keep.ts", "\\#literal.ts"]
        );
        assert!(captured.is_current(&prefs, Some(project)));
        assert!(!captured.is_current(&prefs, Some(Path::new("/fixture/project-linked-worktree"))));
        prefs.set_option(
            Some(project),
            "search.exclude",
            serde_json::json!("different/"),
        );
        assert!(!captured.is_current(&prefs, Some(project)));
        prefs.remove_option(Some(project), "search.exclude");
        assert_eq!(
            Exclusions::read(&prefs, Some(project)).patterns(),
            vec!["vendor/"]
        );
    }
}
