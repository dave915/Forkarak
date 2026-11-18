# Read-only Git backend

The public API in `src/lib.rs` inspects a canonical worktree root, including
linked worktrees and detached or unborn HEADs. No read operation refreshes the
index, writes objects, checks out files, prunes worktrees, starts a language
server, fetches missing objects, or runs repository filters or hooks.

- `Uncommitted` compares HEAD with working contents and retains entries whose
  staged and unstaged changes cancel each other. The flags describe index vs
  HEAD and working contents vs index separately.
- `Branch(base)` compares `merge-base(base, HEAD)` with working contents.
  Committed, staged, unstaged, and nonignored untracked changes are included.
  Invalid/unrelated bases and unborn branch comparisons return actionable errors.
- Worktree inventory counts always describe uncommitted changes. The selected
  comparison's entries and line counts are in `Snapshot.changes`.
- `snapshot_current` refreshes only the selected worktree's working contents and
  diffs. Other registered worktrees remain inventory-only rows with
  `changed_files_available = false`; their zero placeholder is not a clean count.
  Full `snapshot` still inspects sibling counts, but an unreadable sibling is
  reported through its `inspection_error` without blocking the selected reader.
  Successful counts set `changed_files_available = true`.
- `repository_watch_paths` returns the canonical worktree, its actual Git
  directory, and its common Git directory, deduplicated. This includes linked
  worktree indexes/HEAD and shared refs outside the working directory. Discovery
  reads metadata only and preserves native paths, including embedded newlines.
- `Snapshot.files` contains existing tracked and nonignored untracked files,
  including configuration files and symlinks. Deleted files and submodule
  directories are excluded. Gitlinks are opaque index entries; nested submodule
  dirtiness is not inspected.
- Snapshots use NUL-delimited Git records and native paths. Fingerprints include
  streamed content hashes, index stages, HEAD, baseline, branches, and worktree
  inventory. Same-length changes and edits beyond the preview limit invalidate
  them. Reads can race external edits; a detected mid-read change returns a
  refresh-and-retry error.
- Diff line numbers are **one-based**. Search locations have canonical absolute
  paths, **zero-based** lines, and **UTF-16** columns. Search is literal and case
  sensitive, returns individual occurrences, and honors the requested limit
  up to 10,000 results. Empty/multiline queries return no results.

Status and working diffs apply Git's intrinsic text EOL attributes and
`core.autocrlf`. Auto conversion respects binary contents and CRLF already in
the index. Whole-file reads, save preconditions and refresh fingerprints retain
the exact disk bytes. Other content transformations such as external filters,
textconv, ident expansion and working-tree encoding are outside this comparison.
NUL-containing and non-UTF-8 contents are binary. Symlinks show the link target
spelling and never follow the target; search skips symlinks. Traversal outside
the root, Git administration paths, symlink ancestors, and special files cannot
be used to read outside content or block on a FIFO.

Memory is bounded per file: at most 2 MiB from each diff side is retained, with
a 50,000-line diff limit and a 200 ms diff-algorithm deadline. Oversized diffs
return bounded text previews and `truncated = true`; their line counts are zero.
`read_file` rejects oversized files, and search skips them. Metadata output is
capped at 32 MiB and produces an error instead of a partial inventory. Hashing
still streams the entire file using a 64 KiB buffer.

Rename matching first uses identical blobs and staged move identities. Edited
renames then use shared-line similarity for at most 16 additions and 16 deletions
with files up to 256 KiB / 10,000 lines. Larger unmatched sets remain explicit
adds/deletes; their contents are still available. This bounds rename work and
avoids executing external diff drivers.

The integration suite creates temporary repositories and real linked worktrees:

```sh
PATH=/Users/han/.cargo/bin:$PATH cargo test -p pokarak-core
PATH=/Users/han/.cargo/bin:$PATH cargo clippy -p pokarak-core --all-targets -- -D warnings
```

On Herdr, run those commands with `hc run --bg`, then collect completion with
`hc wait` and output with `hc read`.
