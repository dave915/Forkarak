# Pokarak engineering

Read PRODUCT.md and DESIGN.md. Native macOS Rust/GPUI application; do not replace with a webview.

## Work boundaries
- UI: crates/app. Read-only Git calls belong in crates/core. Language server integration belongs in crates/navigation.
- Shared public types and function signatures in the initial scaffold are an integration contract. Coordinate before changing them; additive helpers are fine.
- All Git commands must be argv-based, use `--` for paths, and never mutate a user's repository for viewing.
- Worktree state, navigation and stale async responses must be isolated by worktree path.
- File viewing must not wait for language servers. No implicit project builds or language server startup before trust.
- Text matches are not semantic references. Label fallback results clearly.
- Commit by feature. Use real fixture repositories for backend tests, not user repositories.
- Each worker edits only its assigned crate or script directory, commits its changes, and reports the commit hash. Do not spawn further agents.
- Cargo is installed at /Users/han/.cargo/bin; set PATH explicitly in shell commands.
- Long builds/tests use `hc run --bg` and `hc wait` per installed `hc --skill`.
- The shared build cache is /Users/han/dev/pokarak/target; use `CARGO_TARGET_DIR` if useful, but builds serialize on its lock. Do not delete it.

## App behavior
Worktree list, comparison baseline, changed files, unified diff, whole-file read-only editor, file/text search, back/forward, explicit language analysis, and external-change notification. Use real filesystem data. Save local UI state outside the inspected repository.

