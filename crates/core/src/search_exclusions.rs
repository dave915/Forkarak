//! In-memory rules only: never load ignore files, global settings, or walk the
//! filesystem. Candidate discovery remains the existing read-only Git backend.
use anyhow::{Context, Result, ensure};
use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
};
use std::path::Path;

pub(crate) struct SearchExclusions(Vec<Gitignore>);

impl SearchExclusions {
    pub(crate) fn compile(patterns: &[String]) -> Result<Self> {
        let mut rules = Vec::with_capacity(patterns.len());
        for (index, pattern) in patterns.iter().enumerate() {
            let context = || {
                format!(
                    "Invalid search exclusion pattern #{} {pattern:?}; correct this rule in search.exclude",
                    index + 1
                )
            };
            ensure!(
                !pattern.contains(['\0', '\n', '\r']),
                "{}: each rule must be one line without NUL characters",
                context()
            );
            // A relative matcher root avoids matching absolute worktree parents
            // (including the main repository of a linked worktree).
            let mut builder = GitignoreBuilder::new("");
            builder.allow_unclosed_class(false);
            builder.add_line(None, pattern).with_context(context)?;
            let rule = builder.build().with_context(context)?;
            if !rule.is_empty() {
                rules.push(rule);
            }
        }
        Ok(Self(rules))
    }

    /// Input is already normalized and validated by filesystem::relative.
    pub(crate) fn excludes(&self, path: &Path) -> bool {
        // Keep one matcher per rule and inspect in reverse order so a later
        // directory exclusion can override an earlier specific reinclusion.
        // Combining rules with matched_path_or_any_parents would prefer a
        // direct file match over a later parent rule. No directory pruning:
        // explicit child reinclusions must remain discoverable.
        for rule in self.0.iter().rev() {
            match rule.matched_path_or_any_parents(path, false) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => (),
            }
        }
        false
    }
}
