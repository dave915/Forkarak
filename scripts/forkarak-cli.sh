#!/bin/bash
# Installed with a small preamble containing its app and command locations.
set -euo pipefail
APP_PATH="${FORKARAK_APP_PATH:-${HOME}/Applications/Forkarak.app}"
BIN_DIR="${FORKARAK_BIN_DIR:-${HOME}/.local/bin}"
fail() { echo "Error: $*" >&2; exit 1; }

case "${1:-}" in
    ctl)
        EXECUTABLE="$APP_PATH/Contents/MacOS/forkarak"
        [[ -f "$EXECUTABLE" && -x "$EXECUTABLE" ]] || fail "Forkarak executable is missing or not executable at $EXECUTABLE; run install.sh again"
        # Headless commands own their arguments, cwd, streams and exit status.
        exec "$EXECUTABLE" "$@" ;;
    -h|--help|help)
        cat <<'USAGE'
Forkarak — native Git worktree review
Usage: forkarak [PROJECT ...]
       forkarak -- PROJECT ...
       forkarak ctl COMMAND [OPTIONS ...]
       forkarak update [--version vX.Y.Z]
       forkarak --version

ctl runs the installed AI CLI directly, without opening the GUI, and returns JSON.
  forkarak ctl --help
  forkarak ctl status --repo .
  forkarak ctl instructions --agent claude
  forkarak ctl instructions --agent codex
Agent guide commands return Markdown in result.instructions inside the JSON response.
Use -- before literal project names such as ctl, update, or names starting with -.
Quit Forkarak before updating. Project paths may contain spaces or Korean text.
USAGE
        exit 0 ;;
    --version)
        [[ $# == 1 ]] || fail "--version takes no arguments"
        /usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$APP_PATH/Contents/Info.plist"
        exit 0 ;;
    update)
        shift
        exec /bin/bash "$APP_PATH/Contents/Resources/install.sh" --app-dir "$(dirname "$APP_PATH")" --bin-dir "$BIN_DIR" "$@" ;;
esac

[[ -d "$APP_PATH" ]] || fail "Forkarak is not installed at $APP_PATH; run install.sh again"
PROJECTS=()
PATHS_ONLY=false
for path in "$@"; do
    if ! "$PATHS_ONLY" && [[ "$path" == -- ]]; then PATHS_ONLY=true; continue; fi
    if ! "$PATHS_ONLY" && [[ "$path" == -* ]]; then fail "Unknown option: $path (use -- before literal directory names)"; fi
    case "$path" in
        /*) PROJECTS+=("$path") ;;
        *) PROJECTS+=("$PWD/$path") ;;
    esac
done
# A fresh process is needed so an already open app receives CLI project arguments.
# The app itself restores its saved state when invoked without project paths.
if [[ ${#PROJECTS[@]} == 0 ]]; then
    exec /usr/bin/open "$APP_PATH"
fi
exec /usr/bin/open -n "$APP_PATH" --args -- "${PROJECTS[@]}"
