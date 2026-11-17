#!/bin/bash
# Installed with a small preamble containing its app and command locations.
set -euo pipefail
APP_PATH="${POKARAK_APP_PATH:-${HOME}/Applications/Pokarak.app}"
BIN_DIR="${POKARAK_BIN_DIR:-${HOME}/.local/bin}"
fail() { echo "Error: $*" >&2; exit 1; }

case "${1:-}" in
    -h|--help|help)
        cat <<'USAGE'
Pokarak — native Git worktree review
Usage: pokarak [PROJECT ...]
       pokarak update [--version vX.Y.Z]
       pokarak --version

Quit Pokarak before updating. Project paths may contain spaces or Korean text.
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

[[ -d "$APP_PATH" ]] || fail "Pokarak is not installed at $APP_PATH; run install.sh again"
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
