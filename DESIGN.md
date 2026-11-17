# Pokarak native design

Mode: operate/read. A compact developer review surface, with a steel spork mark and a cool blue-gray palette. The existing conversation supplies the product brief and authorizes the build.

## Foundation
- Ground #20252c, sidebar #1b2026, raised surface #282f38, separator #343e49.
- Main ink #e5eaf0, secondary #a4b0be, accent #7fb8e2. Additions #83c4a1 on #24382f; deletions #e5a0a5 on #3c2a30.
- UI type: IBM Plex Sans; code: JetBrains Mono. Korean falls back to the OS Korean sans. Body 13px, code 13px, panel title 12px, product 17px.
- Spacing 4/8/12/16/24; compact rows 28px, toolbar 36px, controls 28px. Controls radius 5px; main panes are flat and separated by a rule.
- Native keyboard conventions, visible focus, tooltips, drag-resizable panes. No dashboard cards or decorative metrics.

## First viewport
A 36px native titlebar combines macOS window controls, the spork mark, project tabs and open-project action, Graph/Code selection, Stage / Commit, Git actions, refresh and settings. Smaller windows preserve the action labels and collapse secondary icons. Code View adds back and forward navigation.
Repositories open in Git Graph mode. Left 210px: worktrees/branches. The remaining area has a vertically resizable commit graph above the selected commit detail. Graph rows show branch labels, message, author, date and hash; a working-changes row opens the current files. The lower detail has a resizable changed-file list and read-only historical diff/code, with explicit merge-parent selection and guarded cherry-pick/revert actions. The file toolbar offers unified and synchronized side-by-side diff, History, and Blame.

Code View retains the existing changed files/all files, search and current-file reader, including per-worktree reading positions and language navigation. File/text search switches to Code View. Historical editors never invoke current-source language navigation. Project/worktree opens return to the graph and reject old asynchronous history results. Footer: worktree path and loading/analysis status. The worktree sidebar has no explanatory footer caption.

## Signature behavior
Each worktree restores its exact reading context. The current comparison and worktree stay visible as the code reader moves through definitions and back. New filesystem data prompts refresh without yanking the current reading position.

## Empty and multiple projects
Closing all projects restores the welcome screen with project-open action and recent-project list. Worktree sessions, search and reading positions remain isolated within each project. Project close never deletes a repository or worktree.

## Native verification
Inspect the actual macOS window at 1440×900 and a smaller window. Web-only detectors do not apply. Check open/empty/error/loading, long file paths, keyboard navigation, diff/code switch, worktree isolation, and external modifications.
