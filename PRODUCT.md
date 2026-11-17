# Forkarak · 포카락
<!-- impeccable:product-schema 1 -->

## Platform
Native desktop, macOS first on the user's Apple Silicon development machine.

## Stack
User selected Rust and delegated the UI choice. The confirmed UI is GPUI + GPUI Kit, with no webview. Language intelligence runs in separate language server processes.

## Users
A Java/Kotlin/Spring developer who also reads frontend code and uses AI agents across Git worktrees.

## Product Purpose
Forkarak is a native macOS Git client for exploring commit graphs, comparing worktrees, and staging individual files, hunks or lines. Developers can inspect AI-generated changes, follow definitions and usages, and return to the original review location.

## Operating Context
Local Git repositories and multiple worktrees. AI or an external IDE can continue modifying files during review. Users need to inspect both uncommitted changes and changes already committed on a branch.

## Capabilities and Constraints
- Multiple project tabs in one window, per-project worktrees, individual/all-project close, and a recent-project welcome screen.
- Worktree discovery and switching without checkout or mutation.
- Separate file tabs, selected file, scroll position, panel visibility, and navigation history per worktree.
- Multiple native terminal sessions per worktree, with tab selection, individual close/restart, and live-session retention while hidden.
- Code View compares working files with HEAD; explicit CLI branch comparisons accept a chosen base. Graph View provides commit and branch history.
- Changed files, collapsible file trees, whole-file browsing, syntax highlighting and text search.
- Explicit working-file editing and atomic save with external-change checks. Unsaved drafts are isolated per worktree/file and retained outside the inspected repository. Historical files remain read-only.
- A JSON CLI for AI agents: repository inspection, explicitly confirmed save/stage/commit/merge, and private local GUI navigation. Settings provides copyable Claude and Codex launch commands.
- Java/Kotlin, JS/TS/JSX/TSX and frontend/configuration file viewing.
- Cmd-click definitions and explicit references action through language servers. Semantic results and text candidates must be distinguished.
- File and diff viewing must work without a language server or project indexing.
- No unsolicited build/import execution for untrusted repositories.
- Explicit Git actions for branches, remotes, staging, commit/amend, stash and conflict resolution. Repository inspection and worktree viewing remain read-only.
- The default commit graph supports repository-wide commit search, read-only worktree and ref navigation, and parent-relative file diffs. Branch menus offer merge and rebase. Commit menus offer single-commit cherry-pick and revert.
- The graph's working-changes row supports line, hunk and file staging and unstaging, a per-worktree commit draft and commit submission. Advanced history editing and Spring-specific analysis are later scope.

## Brand Commitments
The English name is Forkarak and the Korean name is 포카락. The mark is a spork with a spoon bowl and short fork tines.

## Product Principles
1. Preserve the reading location across worktrees and live updates.
2. Always show the worktree and comparison baseline.
3. Never resolve historical code silently against a different version.
4. Be usable before indexing, with honest analysis status.
5. Use real repository data; never display invented activity.

## Open Decisions
Language server compatibility and memory budgets need empirical checks. Initial macOS target follows the available machine; other OS packaging is not yet verified.
