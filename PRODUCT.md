# 포카락
<!-- impeccable:product-schema 1 -->

## Platform
Native desktop, macOS first on the user's Apple Silicon development machine.

## Stack
User selected Rust and delegated the UI choice. The confirmed UI is GPUI + GPUI Kit, with no webview. Language intelligence runs in separate language server processes.

## Users
A Java/Kotlin/Spring developer who also reads frontend code and uses AI agents across Git worktrees.

## Product Purpose
Quickly understand AI-produced code changes, inspect surrounding code, follow definitions and usages, and return to the original review location.

## Operating Context
Local Git repositories and multiple worktrees. AI or an external IDE can continue modifying files during review. Users need to inspect both uncommitted changes and changes already committed on a branch.

## Capabilities and Constraints
- Multiple project tabs in one window, per-project worktrees, individual/all-project close, and a recent-project welcome screen.
- Worktree discovery and switching without checkout or mutation.
- Separate file tabs, selected file, scroll position, and navigation history per worktree.
- Working-tree versus HEAD and branch changes versus a chosen base.
- Changed files, whole-file browsing, syntax highlighting and text search.
- Java/Kotlin, JS/TS/JSX/TSX and frontend/configuration file viewing.
- Cmd-click definitions and explicit references action through language servers. Semantic results and text candidates must be distinguished.
- File and diff viewing must work without a language server or project indexing.
- No unsolicited build/import execution for untrusted repositories.
- Explicit Git actions for branches, remotes, staging, commit/amend, stash and conflict resolution. Repository inspection and worktree viewing remain read-only.
- Commit graph with parent-relative file diffs and single-commit cherry-pick/revert. Advanced history editing and Spring-specific analysis are later scope.

## Brand Commitments
Name: 포카락. Project/English spelling: pokarak. Mark: the user's spork reference, a spoon ending in short fork tines.

## Product Principles
1. Preserve the reading location across worktrees and live updates.
2. Always show the worktree and comparison baseline.
3. Never resolve historical code silently against a different version.
4. Be usable before indexing, with honest analysis status.
5. Use real repository data; never display invented activity.

## Open Decisions
Language server compatibility and memory budgets need empirical checks. Initial macOS target follows the available machine; other OS packaging is not yet verified.
