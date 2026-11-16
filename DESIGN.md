# Pokarak native design

Mode: operate/read. A compact developer review surface, with a steel spork mark and a cool blue-gray palette. The existing conversation supplies the product brief and authorizes the build.

## Foundation
- Ground #20252c, sidebar #1b2026, raised surface #282f38, separator #343e49.
- Main ink #e5eaf0, secondary #a4b0be, accent #7fb8e2. Additions #83c4a1 on #24382f; deletions #e5a0a5 on #3c2a30.
- UI type: IBM Plex Sans; code: JetBrains Mono. Korean falls back to the OS Korean sans. Body 13px, code 13px, panel title 12px, product 17px.
- Spacing 4/8/12/16/24; compact rows 28px, toolbar 44px, controls 28px. Controls radius 5px; main panes are flat and separated by a rule.
- Native keyboard conventions, visible focus, tooltips, drag-resizable panes. No dashboard cards or decorative metrics.

## First viewport
Top toolbar: product mark/name, repository name, open repository, refresh, navigation, analysis settings.
Left 210px: worktrees with branch, local path and change count. Middle 270px: changed files/all files and search. Remaining width: selected file header, diff/code modes, native code surface. Footer: worktree path, comparison, loading/analysis status.

## Signature behavior
Each worktree restores its exact reading context. The current comparison and worktree stay visible as the code reader moves through definitions and back. New filesystem data prompts refresh without yanking the current reading position.

## Native verification
Inspect the actual macOS window at 1440×900 and a smaller window. Web-only detectors do not apply. Check open/empty/error/loading, long file paths, keyboard navigation, diff/code switch, worktree isolation, and external modifications.
