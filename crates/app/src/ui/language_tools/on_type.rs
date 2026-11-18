use super::*;

/// The editor reports committed buffer changes. Restrict automatic queries to a
/// single insertion at the caret, including the editor's paired parentheses.
pub(in crate::ui) fn inserted_character(before: &str, after: &str, caret: usize) -> Option<char> {
    let (prefix, suffix) = after.split_at_checked(caret)?;
    let typed = prefix.chars().next_back()?;
    let start = caret - typed.len_utf8();
    let old_suffix = before.strip_prefix(&prefix[..start])?;
    (old_suffix == suffix || (typed == '(' && suffix.strip_prefix(')') == Some(old_suffix)))
        .then_some(typed)
}

fn input_tool(typed: char, signatures: bool) -> Option<Tool> {
    if matches!(typed, '(' | ',') && signatures {
        Some(Tool::Signature)
    } else if typed.is_alphanumeric() || matches!(typed, '_' | '$' | '.' | ':') {
        Some(Tool::Complete)
    } else {
        None
    }
}

/// Prefer candidates matching the word before the caret. Preserve the server's
/// order among equally good matches and keep explicit requests unfiltered.
pub(super) fn rank_input_candidates(
    items: &mut pokarak_navigation::CompletionResult,
    source: &str,
    caret: usize,
) {
    let Some(before) = source.get(..caret) else {
        return;
    };
    let prefix = before
        .rsplit(|ch: char| !(ch.is_alphanumeric() || matches!(ch, '_' | '$')))
        .next()
        .unwrap_or_default()
        .to_lowercase();
    if prefix.is_empty() {
        return;
    }
    let mut ranked: Vec<_> = items
        .items
        .drain(..)
        .filter_map(|item| {
            let rank = match_rank(&prefix, &item.label)
                .or_else(|| match_rank(&prefix, &item.insert_text))?;
            Some((rank, item))
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    items.items.extend(ranked.into_iter().map(|(_, item)| item));
}

fn match_rank(prefix: &str, name: &str) -> Option<u8> {
    let name = name.to_lowercase();
    if name.starts_with(prefix) {
        return Some(0);
    }
    if name.contains(prefix) {
        return Some(1);
    }
    let mut letters = name.chars();
    prefix
        .chars()
        .all(|ch| letters.by_ref().any(|candidate| ch == candidate))
        .then_some(2)
}

impl Pokarak {
    pub(super) fn can_show_input_assistance(
        &self,
        key: &FileKey,
        editor: &Entity<EditorState>,
        tool: &Tool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let project = self.current_project.as_deref();
        self.navigation.is_some()
            && self.language_trusted()
            && self.prefs.option_bool(project, "language.unsaved", true)
            && match tool {
                Tool::Complete => {
                    self.prefs.option_bool(project, "language.completion", true)
                        && self
                            .prefs
                            .option_bool(project, "language.completion_on_input", true)
                }
                Tool::Signature => {
                    self.prefs
                        .option_bool(project, "language.auto_signature_help", false)
                }
                _ => false,
            }
            && pokarak_navigation::language_for_path(&key.path).is_some_and(|language| {
                self.managed_language_enabled(language)
                    && (self.language_tools.input_languages.contains(language)
                        || self
                            .language_tools
                            .input_languages
                            .contains(pokarak_navigation::language_family(language)))
            })
            && self
                .language_target(cx)
                .is_some_and(|(active, state)| active == *key && state == *editor)
            && editor.read(cx).focus_handle(cx).is_focused(window)
            && editor.read(cx).selected_range().is_empty()
            && !self.git_is_busy()
            && !self.active_worktree_has_git_job()
            && !self.trust_prompt
            && !window.has_active_dialog(cx)
    }

    pub(in crate::ui) fn schedule_input_assistance(
        &mut self,
        key: &FileKey,
        editor: &Entity<EditorState>,
        typed: Option<char>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.language_tools.on_type_task = None;
        let signatures = self.prefs.option_bool(
            self.current_project.as_deref(),
            "language.auto_signature_help",
            false,
        );
        let Some(tool) = typed.and_then(|ch| input_tool(ch, signatures)) else {
            return;
        };
        if !self.can_show_input_assistance(key, editor, &tool, window, cx) {
            return;
        }
        let editor = editor.clone();
        let key = key.clone();
        let caret = editor.read(cx).cursor();
        let view = self.language_tools.view;
        let timer = cx.background_executor().timer(Duration::from_millis(150));
        self.language_tools.on_type_task = Some(cx.spawn_in(window, async move |app, cx| {
            timer.await;
            let _ = app.update_in(cx, |app, window, cx| {
                if app.language_tools.view == view
                    && editor.read(cx).cursor() == caret
                    && app.can_show_input_assistance(&key, &editor, &tool, window, cx)
                {
                    app.run_language_request(tool, None, true, window, cx);
                }
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::{Tool, input_tool, inserted_character, match_rank};

    #[test]
    fn detects_committed_single_input_at_the_caret() {
        assert_eq!(inserted_character("foo.bar", "foo.한bar", 7), Some('한'));
        assert_eq!(inserted_character("foo\n", "foo()\n", 4), Some('('));
        assert_eq!(inserted_character("ab", "ab", 2), None);
        assert_eq!(inserted_character("a", "abcd", 4), None);
        assert_eq!(inserted_character("abc", "ac", 1), None);
        assert_eq!(inserted_character("abc", "axc", 2), None);
        assert_eq!(inserted_character("ax", "axy", 1), None);
        assert_eq!(inserted_character("", "한", 1), None);
    }

    #[test]
    fn candidate_matching_preserves_prefix_priority_and_camel_case_abbreviations() {
        assert_eq!(match_rank("rep", "replace"), Some(0));
        assert_eq!(match_rank("rep", "toRepr"), Some(1));
        assert_eq!(match_rank("gsv", "getServerVersion"), Some(2));
        assert_eq!(match_rank("rep", "length"), None);
    }

    #[test]
    fn routes_only_language_input_and_opted_in_signatures() {
        for ch in ['v', '_', '.', ':', '한'] {
            assert!(matches!(input_tool(ch, false), Some(Tool::Complete)));
        }
        for ch in ['(', ','] {
            assert!(matches!(input_tool(ch, true), Some(Tool::Signature)));
            assert!(input_tool(ch, false).is_none());
        }
        assert!(input_tool(' ', true).is_none());
    }
}
