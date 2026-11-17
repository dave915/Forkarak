# Forkarak native design

Mode: operate/read. A compact developer review surface, with a steel spork mark and a cool blue-gray palette. The existing conversation supplies the product brief and authorizes the build.

## Foundation
- Ground #20252c, sidebar #1b2026, raised surface #282f38, separator #343e49.
- Main ink #e5eaf0, secondary #a4b0be, accent #7fb8e2. Additions #83c4a1 on #234535; deletions #e5a0a5 on #492d35.
- UI type: IBM Plex Sans; code: JetBrains Mono. Korean falls back to the OS Korean sans. Body 13px, code 13px, panel title 12px, product 17px.
- Spacing 4/8/12/16/24; compact rows 28px, toolbar 36px, controls 28px. Controls radius 5px; main panes are flat and separated by a rule.
- Native keyboard conventions, visible focus, tooltips, drag-resizable panes. No dashboard cards or decorative metrics.

## First viewport
The 36px native titlebar keeps project tabs and the open-project action on the left, followed by a subtly raised toolbar with seven icon buttons for Stage / Commit, branches, stash, fetch, pull, push and more Git actions. The same buttons stay visible at the minimum window width. Hover reveals each action's name, and each icon has an accessibility label. The Graph/Code selector sits on the right, followed by the terminal, refresh and settings icons. Code View adds back and forward navigation immediately before the selector. The terminal button toggles a resizable bottom panel running an interactive login zsh for the selected worktree. Hiding the panel keeps the session alive.

Repositories open in Git Graph mode. A collapsible group for each of worktrees, local branches, remotes, tags and stashes sits to the left of the graph. Selecting a reference reveals its commit. Each commit table header has a draggable trailing edge. Header and rows scroll together horizontally when their widths exceed the viewport. The branch selector filters local and remote names and scopes the graph, search and pagination to the selected branch. Scrolling near the end loads the next 100 commits in that scope. Branch hover follows its first parent through merges. Cmd-F searches commit messages, authors and hashes across the repository. A commit context menu offers cherry-pick and revert. Local branch context menus name the source and destination for merge and rebase.

Selecting a commit opens its changed files and read-only diff below the graph. The file toolbar offers unified and synchronized side-by-side diff, History and Blame, with parent selection for merges. Selecting the working-changes row opens staging, a diff and a commit message in the same pane. Each worktree remembers its branch filter, column widths, side-by-side diff ratio, pane dimensions and collapsed sections.

Code View retains the existing changed files/all files, search and current-file reader, including per-worktree reading positions and language navigation. Settings adjusts interface and code font sizes independently and provides an application-wide zoom. Unified and side-by-side patches keep the language colors of Code View. Background tints and gutter signs identify additions and deletions in both themes. The two sides use their own paths and source snapshots so a rename or a multiline comment can change how the same text is colored. File/text search switches to Code View. Historical editors never invoke current-source language navigation. Project/worktree opens return to the graph and reject old asynchronous history results. Footer: worktree path and loading/analysis status. The worktree sidebar has no explanatory footer caption.

## Signature behavior
Each worktree restores its exact reading context. The current comparison and worktree stay visible as the code reader moves through definitions and back. New filesystem data prompts refresh without yanking the current reading position.

## Empty and multiple projects
Closing all projects restores the welcome screen with project-open action and recent-project list. Worktree sessions, search and reading positions remain isolated within each project. Project close never deletes a repository or worktree.

## In-app updates
The installed app checks for stable releases in the background. A compact banner on the welcome screen or workspace offers the download. Settings also shows the installed version and a manual check. Download and verification leave the window usable. Once ready, the user chooses when to restart. The app remembers the active worktree, reading view and commit drafts, and asks for confirmation if restarting would end a live terminal or discard conflict-editor input.

## Native verification
Inspect the actual macOS window at 1440×900 and a smaller window. Web-only detectors do not apply. Check open/empty/error/loading, long file paths, keyboard navigation, diff/code switch, worktree isolation, and external modifications.
