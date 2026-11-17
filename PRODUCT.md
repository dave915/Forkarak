# Forkarak · 포카락
<!-- impeccable:product-schema 1 -->

## Platform
Native desktop, macOS first on the user's Apple Silicon development machine.

## Stack
User selected Rust and delegated the UI choice. The confirmed UI is GPUI + GPUI Kit, with no webview. Language intelligence runs in separate language server processes.

## Users
A Java/Kotlin/Spring developer who also edits frontend and configuration files, moves between Git worktrees, and uses external AI agents during development.

## Product Purpose
Forkarak is a lightweight native macOS code editor and Git workspace. Developers can read and edit code, inspect changes, stage and commit them, and move between worktrees in the same window while keeping their working context.

The central workflow is code editing ↔ change inspection ↔ worktree switching. Code navigation, terminals and optional AI tools support that workflow. Developers should be able to choose an editor or Git view as their default working screen.

## Operating Context
Local Git repositories and multiple worktrees. AI or an external IDE can continue modifying files while the developer reads or edits them. Users need to work with both uncommitted changes and changes already committed on a branch, without opening a separate editor window for each worktree.

Herdr is the user's primary environment for AI work spanning projects and other tasks. Forkarak provides a lightweight way to invoke Claude or Codex for the current worktree when needed. AI integration follows the developer's explicit actions and keeps the target worktree visible.

## Current Capabilities and Constraints

- Multiple project tabs in one window, drag reordering in the tab bar and project list with persisted shared order, per-project worktrees, individual/all-project close, and a recent-project welcome screen.
- Worktree discovery and switching without checkout or mutation.
- Separate file tabs, selected file, scroll position, panel visibility, and navigation history per worktree.
- Project and worktree opens follow the selected Editor/Git default or restore the last working screen, file and reading position when enabled.
- Multiple native terminal sessions per worktree, with tab selection, individual close/restart, and live-session retention while hidden.
- Code View defaults to working files versus HEAD. Explicit GUI and CLI branch comparisons use merge-base(base, HEAD), include committed and working changes, and retain their applied baseline per worktree. Graph branch filtering remains independent.
- External file, commit and reference changes refresh automatically after a quiet interval. Active editing, saves, Git operations, dialogs and marked staging selections defer the queued refresh. Selected files, drafts and code/diff/graph reading positions are preserved.
- Selected-worktree refreshes avoid reading sibling working files. Full inspections isolate sibling read failures and distinguish unavailable dirty counts from zero.
- Changed files, collapsible file trees, whole-file browsing, syntax highlighting and text search.
- Explicit working-file editing and atomic save with external-change checks. Unsaved drafts are isolated per worktree/file and retained outside the inspected repository. A three-source reconciliation view compares original, draft and current disk; applying changes only the retained draft, with a separate explicit Save. Historical files remain read-only.
- File review checkpoints are isolated by worktree, path and comparison context, persisted outside the repository, and verified against exact contents. Changed contents invalidate the mark; unrelated file changes do not.
- Unified and side-by-side review diffs highlight changed words, optionally hide whitespace-only changes, and navigate between changed hunks. Display filtering never alters raw patches or staging coordinates.
- A JSON CLI for AI agents: repository inspection, explicitly confirmed save/stage/commit/merge, and private local GUI navigation. Settings provides copyable Claude and Codex launch commands.
- Java/Kotlin, JS/TS/JSX/TSX and frontend/configuration file viewing.
- Cmd-click definitions and explicit references action through language servers. Semantic results and text candidates must be distinguished.
- File and diff viewing must work without a language server or project indexing.
- No unsolicited build/import execution for untrusted repositories.
- Explicit Git actions for branches, remotes, staging, commit/amend, stash and conflict resolution. Repository inspection and worktree viewing remain read-only.
- Upstream-aware Push/Pull forms, explicit push destinations and first-push tracking setup. Local ahead/behind counts and this worktree's last Fetch time distinguish locally known metadata from the server's latest state.
- Branch creation from a selected reference or commit, optional create-and-switch and remote tracking, and local branch rename with target validation.
- The default commit graph supports repository-wide commit search, read-only worktree and ref navigation, and parent-relative file diffs. Branch menus offer merge and rebase. Commit menus offer single-commit cherry-pick and revert.
- The graph's working-changes row supports line, hunk, individual-file and selected-files staging and unstaging, a per-worktree commit draft and commit submission.
- Explicit Stash Apply and Pop actions identify the selected shared stash and destination worktree. Conflict resolution compares Ours and Theirs together, offers Base, navigates conflict regions and files, and retains in-session resolution drafts. Saving and staging the result is explicit.
- Worktree-scoped Git operation history retains successful and failed output for the current app session. Current limits are 20 operations per worktree and 32 KiB per output stream; history is cleared on exit.

## 1.0.0 Product Experience

The detailed scope is maintained in [the improvement backlog](docs/improvement-backlog.md): 64 feature/UX items and 35 settings items. The sections below describe the integrated 1.0.0 experience. The backlog retains the original requirement numbering; language-server and platform limitations remain documented separately.

### Default Working Screen and Restoration

- Choose Editor mode (Code View and file tree) or Git mode (commit graph and change inspection) as the application's default working screen, with per-project overrides.
- Offer a separate setting to restore the last active screen, file, cursor and reading position per worktree. When restoration is enabled and a saved context exists, restore it; otherwise use the project's chosen default screen.
- Keep the choice of working screen separate from the choice to open working files in read mode or enter editing directly. Historical sources remain read-only.

### Everyday Editing and Navigation

- Add file/folder creation, rename and move; comment toggling; line move/duplicate; repeated selection of matching words; go-to-line; and a soft-wrap toggle.
- Apply `.editorconfig` and detected file indentation before fallback tab/space settings. Resolve shortcut conflicts between editing, project search, multiple cursors and worktree navigation.
- Provide a command palette, searchable file symbols, reusable preview tabs, inline change indicators with previous-code expansion, and grouped source excerpts for search and usage results.
- Navigate directly to a single semantic definition and show an anchored chooser for multiple results. Keep definition and usage commands distinct. Offer usage results in a popover, with optional pinning in the files area or a bottom panel.
- Display Blame beside the code with commit details on demand. Preserve the original reading location when following code and returning.
- Extend explicitly enabled language analysis with completion, diagnostics, formatting and navigation over unsaved buffers. Unsaved-buffer analysis must use the matching displayed source and worktree; historical sources must never silently resolve against current code.

### Git Workflow and Change Inspection

- Clarify the active worktree independently of the selected commit. Keep file paths, comparison context and essential actions readable in small windows; reclaim space for the diff during staging.
- Restore hidden search controls when search is invoked, report search failures accurately, disclose result limits, distinguish duplicate file names in tabs, and make close shortcuts predictable.
- Add an unreviewed-files filter and navigation to the next unreviewed file. Explain why a file cannot be marked reviewed and provide the corresponding next action.
- Distinguish authentication, filesystem permission and repository-lock errors. Surface ongoing merge/rebase/cherry-pick/revert state with Continue/Abort actions in the workspace and align commit controls with that state.
- Retain an already-enabled analysis session for Git operations that leave source contents unchanged. Refresh or invalidate source-dependent state when an operation changes the checked-out contents.
- Preview affected commits and files before Push/Merge/Rebase. Add live Git output, explicit cancellation with state reinspection, direct comparison of selected commits/tags/branches, reflog browsing and recovery-branch creation.
- Add `.gitignore` rule editing with affected-path previews, Clone/Init, tag creation/push, and explicit undo of the last commit while retaining working-file changes. Viewing and previewing these actions remain read-only; writes belong to the operations layer.

### Tasks, Logs and Extensions

- Provide saved task profiles with run, stop, restart and status, plus searchable logs and stack-trace links to file locations.
- Add a versioned extension API for commands, settings, formatters and language-server integration. Load required capabilities on demand and isolate extension execution in a separate process or WebAssembly runtime.
- Keep task state, logs and project-specific extension settings isolated by worktree. Forkarak supplies the shared native GPUI surfaces for execution, logs and settings.
- Plan Spring support as the first extension, covering Gradle/Maven execution, JDK and profile configuration, environment variables, and log/stack-trace navigation.

### Settings and Configuration

- Organize appearance, editor, navigation/search, Git/review, keyboard, terminal/tasks, AI tools, extensions and updates into clear categories.
- Show each setting's application, project or worktree scope, with project overrides where applicable. Retain user state and configuration outside inspected repositories.
- Keep theme, application zoom, interface/code font sizes and readable previews; offer an independently configurable terminal font size. Make reset scope and copy confirmation visible.
- Provide the default working-screen and restoration controls described above, editing/indentation preferences, navigation-result placement, review behavior and shortcut conflict management.
- Provide explicit language-server/formatter configuration and analysis-trust management, terminal and task profiles, Spring settings, AI CLI selection and launch options, extension management, and update-check preferences.
- Present compact AI launch/copy controls with a visible worktree target and expandable command details. The chosen launch action uses that worktree's terminal context.

## Brand Commitments
The English name is Forkarak and the Korean name is 포카락. The mark is a spork with a spoon bowl and short fork tines.

## Product Principles

1. Keep editing, change inspection and worktree switching quick, with a user-selected default working screen.
2. Preserve files, drafts, reading locations and navigation context across worktrees and live updates.
3. Always make the active worktree, comparison baseline and write destination clear.
4. Never resolve historical code silently against a different version, and distinguish semantic navigation from text candidates.
5. Be usable before indexing, with honest analysis status and explicit trust before project analysis or imports.
6. Start optional language, task, AI and extension processes when their functionality is explicitly requested.
7. Use real repository data; never display invented activity.
8. Keep repository inspection read-only and make Git writes explicit, including recovery and cancellation outcomes.
9. Retain the native Rust/GPUI application and keep persistent local UI state outside the inspected repository.

## Open Decisions
Language server compatibility, memory budgets and large-repository performance need empirical checks. The exact extension API, execution runtime and delivery sequence remain design work. Initial macOS target follows the available machine; other OS packaging is not yet verified.
