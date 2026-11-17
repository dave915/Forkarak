//! File-kind defaults below explicit transient per-file toggles. Project lookup
//! receives the owning canonical project, independently of the worktree path.
use crate::prefs::Preferences;
use std::path::Path;

pub(super) fn configured(prefs: &Preferences, project: Option<&Path>, path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    prefs
        .option(project, "editor.wrap_patterns")
        .and_then(|value| {
            // Accept the typed settings map, plus legacy serialized JSON values.
            if let Some(text) = value.as_str() {
                serde_json::from_str::<serde_json::Value>(text).ok()
            } else {
                Some(value.clone())
            }
        })
        .and_then(|value| {
            value
                .get(&extension)
                .or_else(|| value.get(format!(".{extension}")))
                .and_then(|value| value.as_bool())
        })
        .unwrap_or_else(|| prefs.option_bool(project, "editor.word_wrap", false))
}

#[cfg(test)]
mod tests {
    use super::configured;
    use crate::prefs::Preferences;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn extension_rules_override_both_true_and_false_global_defaults() {
        let mut prefs = Preferences::default();
        prefs.set_option(None, "editor.word_wrap", json!(false));
        prefs.set_option(None, "editor.wrap_patterns", json!({"md":true,"rs":false}));
        assert!(configured(&prefs, None, Path::new("notes.md")));
        assert!(configured(&prefs, None, Path::new("README.MD")));
        assert!(!configured(&prefs, None, Path::new("src/main.rs")));
        prefs.set_option(None, "editor.word_wrap", json!(true));
        assert!(!configured(&prefs, None, Path::new("src/main.rs")));
        assert!(configured(&prefs, None, Path::new("unknown.txt")));
        prefs.set_option(None, "editor.wrap_patterns", json!({".md":false}));
        assert!(!configured(&prefs, None, Path::new("notes.md")));
    }

    #[test]
    fn linked_worktrees_use_canonical_project_rules_and_other_projects_inherit_app() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().canonicalize().unwrap();
        let linked = project.join("linked-worktree");
        let other = project.join("another-project");
        let mut prefs = Preferences::default();
        prefs.set_option(None, "editor.word_wrap", json!(false));
        prefs.set_option(None, "editor.wrap_patterns", json!({"md":true}));
        prefs.set_option(
            Some(&project),
            "editor.wrap_patterns",
            json!({"md":false,"rs":true}),
        );
        for path in [
            Path::new("src/main.rs").to_owned(),
            linked.join("src/main.rs"),
        ] {
            assert!(configured(&prefs, Some(&project), &path));
            assert!(!configured(&prefs, Some(&other), &path));
        }
        assert!(!configured(&prefs, Some(&project), Path::new("notes.md")));
        assert!(configured(&prefs, Some(&other), Path::new("notes.md")));
        prefs.remove_option(Some(&project), "editor.wrap_patterns");
        assert!(configured(&prefs, Some(&project), Path::new("notes.md")));
    }

    #[test]
    fn legacy_maps_and_invalid_values_fall_back_to_the_effective_default() {
        let mut prefs = Preferences::default();
        prefs.set_option(None, "editor.word_wrap", json!(true));
        prefs.set_option(None, "editor.wrap_patterns", json!("{\"rs\":false}"));
        assert!(!configured(&prefs, None, Path::new("main.rs")));
        for invalid in [json!({"rs":"invalid"}), json!("invalid JSON"), json!([])] {
            prefs.set_option(None, "editor.wrap_patterns", invalid);
            assert!(configured(&prefs, None, Path::new("main.rs")));
        }
    }
}
