#!/bin/bash
# Installed with a small preamble containing its app and command locations.
set -euo pipefail
APP_PATH="${FORKARAK_APP_PATH:-${HOME}/Applications/Forkarak.app}"
BIN_DIR="${FORKARAK_BIN_DIR:-${HOME}/.local/bin}"
fail() { echo "Error: $*" >&2; exit 1; }

case "${1:-}" in
    ctl|install-cli)
        EXECUTABLE="$APP_PATH/Contents/MacOS/forkarak"
        [[ -f "$EXECUTABLE" && -x "$EXECUTABLE" ]] || fail "Forkarak executable is missing or not executable at $EXECUTABLE; run install.sh again"
        # Headless commands own their arguments, cwd, streams and exit status.
        exec "$EXECUTABLE" "$@" ;;
    -h|--help|help)
        cat <<'USAGE'
Forkarak — native macOS code editor and Git worktrees
Usage: forkarak [OPTIONS] [--] [FILE|DIRECTORY ...]
       fkk [OPTIONS] [--] [FILE|DIRECTORY ...]
       forkarak -g FILE:LINE[:COLUMN]
       forkarak ctl COMMAND [OPTIONS ...]
       forkarak install-cli [--bin-dir DIRECTORY]
       forkarak update [--version vX.Y.Z]
       forkarak --version

  -g, --goto FILE:LINE[:COLUMN]  One-based line/Unicode character column
  -r, --reuse-window            Reuse a unique matching window (or sole window)
  -n, --new-window              Always open a new window
      --background, --no-launch Reuse only; never activate or launch an app
      --session SOCKET         Explicit reuse target from ctl sessions
      --session-dir DIR        Isolated application state; scopes reuse
      --window-size WIDTHxHEIGHT  New window size (minimum 940x580)

Existing files/directories in Git worktrees only; . opens a project.
Plain paths preserve colons; with --goto an existing whole filename wins.
Use -- before literal reserved names (ctl, update, help, install-cli) or names starting with -.
Multiple paths/goto flags are allowed. Spaces and Unicode are preserved.
Default: reuse a unique window containing all requested worktrees; otherwise new.
Reused windows are shown unless --background/--no-launch is given. Ambiguous reuse fails; select --session or --new-window.
Loose files/new file creation, --wait and --diff are not supported.

ctl runs directly without opening the GUI and returns JSON:
  forkarak ctl --help
  forkarak ctl status --repo .
  forkarak ctl instructions --agent claude
  forkarak ctl instructions --agent codex
Agent guides return Markdown in result.instructions inside the JSON response.
fkk supports the same ctl, update, help and version commands. Quit Forkarak before updating.
USAGE
        exit 0 ;;
    -v|--version)
        [[ $# == 1 ]] || fail "--version takes no arguments"
        /usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$APP_PATH/Contents/Info.plist"
        exit 0 ;;
    update)
        shift
        exec /bin/bash "$APP_PATH/Contents/Resources/install.sh" --app-dir "$(dirname "$APP_PATH")" --bin-dir "$BIN_DIR" "$@" ;;
esac

EXECUTABLE="$APP_PATH/Contents/MacOS/forkarak"
[[ -f "$EXECUTABLE" && -x "$EXECUTABLE" ]] || fail "Forkarak executable is missing or not executable at $EXECUTABLE; run install.sh again"
# The native headless launcher owns path/goto parsing, private session discovery,
# and LaunchServices argv. Never re-tokenize paths or interpolate shell commands.
exec "$EXECUTABLE" --cli-open "$APP_PATH" "$@"
