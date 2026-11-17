#!/bin/bash
# Sourced for preflight, then executed from the private staging directory.
# Publishing is a single macOS rename swap. The old bundle remains available
# for a second swap until LaunchServices accepts the new bundle.
bundle_identity() {
    /usr/bin/stat -f '%d:%i' "$1" "$1/Contents/MacOS/pokarak"
}

bundle_users() {
    # lsof returns 1 without output when no process maps the executable.
    local executable="$1/Contents/MacOS/pokarak" result rc
    [[ -f "$executable" ]] || return 2
    if result="$(/usr/sbin/lsof -nP -t -a -d txt "$executable" 2>&1)"; then
        [[ "$result" =~ ^[[:digit:][:space:]]+$ ]] || return 2
        printf '%s' "$result"
    else
        rc=$?
        [[ "$rc" -eq 1 && -z "$result" ]]
    fi
}

require_origin() {
    local users
    [[ "$(bundle_identity "$APP_PATH")" == "$ORIGINAL_ID" ]] || fail "The installed app changed during the update"
    users="$(bundle_users "$APP_PATH")" || fail "Cannot verify which processes are using $APP_PATH"
    case "$users" in
        "$WAIT_PID") ;;
        *$'\n'*) fail "Quit other Pokarak instances using $APP_PATH, then retry" ;;
        *) fail "The specified process is not the running Pokarak at $APP_PATH" ;;
    esac
}

swap_bundles() {
    # RENAME_SWAP is part of Darwin. Use the system JavaScript bridge so an
    # installation requires neither compiler tools nor a third-party runtime.
    /usr/bin/osascript -l JavaScript - "$@" <<'JXA'
ObjC.import('Foundation')
ObjC.bindFunction('access', ['int', ['char *', 'int']])
ObjC.bindFunction('renamex_np', ['int', ['char *', 'char *', 'unsigned int']])
function run(paths) {
    // Darwin _DELETE_OK includes ACLs and immutable flags on the destination.
    if (paths.length === 3 && $.access(paths[2], 1 << 12) !== 0)
        throw Error('The installed bundle cannot be replaced. Move it into a writable application directory.')
    if ($.renamex_np(paths[0], paths[1], 2) !== 0)
        throw Error('This volume or its permissions prevented an atomic bundle exchange.')
}
JXA
}

if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then return 0; fi
set -euo pipefail
fail() { echo "Error: $*" >&2; exit 1; }
[[ $# -eq 4 ]] || fail "Start this helper through install.sh"
APP_PATH="$1"
WAIT_PID="$2"
ORIGINAL_ID="$3"
SESSION_DIR="$4"
WORK_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
APP_DIR="${APP_PATH%/*}"
LOCK_DIR="$APP_DIR/.pokarak-install.lock"
STAGED_APP="$WORK_DIR/unpacked/포카락.app"
[[ "${WORK_DIR%/*}" == "$APP_DIR" && "${WORK_DIR##*/}" == .pokarak-install.?????? &&
    -f "$LOCK_DIR/work-dir" && "$(<"$LOCK_DIR/work-dir")" == "$WORK_DIR" ]] || fail "Invalid update staging directory"
ACKNOWLEDGED=false
COMPLETED=false

reopen_app() {
    if [[ -n "$SESSION_DIR" ]]; then
        /usr/bin/open -n "$APP_PATH" --args --session-dir "$SESSION_DIR"
    else
        /usr/bin/open -n "$APP_PATH"
    fi
}

finish() {
    local status=$? keep_stage=false users
    trap - EXIT
    trap '' TERM INT HUP
    if "$ACKNOWLEDGED" && ! "$COMPLETED"; then
        # Detect a completed swap even if we were interrupted immediately after
        # the system call. Never throw away the only copy of the original.
        if [[ "$(bundle_identity "$STAGED_APP" 2>/dev/null)" == "$ORIGINAL_ID" ]]; then
            if [[ "$(bundle_identity "$APP_PATH" 2>/dev/null)" == "$STAGED_ID" ]] &&
                users="$(bundle_users "$APP_PATH")" && [[ -z "$users" ]] && swap_bundles "$APP_PATH" "$STAGED_APP"; then
                echo "Restored the previous Pokarak at $APP_PATH"
            else
                keep_stage=true
                echo "Restore the previous Pokarak from $STAGED_APP" >&2
            fi
        fi
        if [[ "$(bundle_identity "$APP_PATH" 2>/dev/null)" == "$ORIGINAL_ID" ]] &&
            users="$(bundle_users "$APP_PATH")" && [[ -z "$users" ]]; then
            reopen_app || echo "Open $APP_PATH in Finder to resume the previous version." >&2
        fi
    fi
    if ! "$keep_stage" && ! /bin/rm -rf "$WORK_DIR"; then
        echo "Some temporary update files could not be removed from $WORK_DIR" >&2
    fi
    /bin/rm -f "$LOCK_DIR/work-dir"
    /bin/rmdir "$LOCK_DIR"
    exit "$status"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP
trap 'exit 141' PIPE

STAGED_ID="$(bundle_identity "$STAGED_APP")"
/bin/mkdir "$WORK_DIR/swap-a" "$WORK_DIR/swap-b"
swap_bundles "$WORK_DIR/swap-a" "$WORK_DIR/swap-b" "$APP_PATH" || fail "Cannot safely replace Pokarak at $APP_PATH"
umask 077
LOG_DIR="$HOME/Library/Logs/Pokarak"
/bin/mkdir -p "$LOG_DIR" || fail "Cannot create the update log directory"
LOG="$LOG_DIR/update.log"
[[ ! -L "$LOG" && ( ! -e "$LOG" || -f "$LOG" ) ]] || fail "Cannot open the update log at $LOG"
exec 4>>"$LOG" || fail "Cannot write the update log at $LOG"
require_origin
echo "Update log: $LOG"
echo "POKARAK_UPDATE_READY"
IFS= read -r decision || exit 0
[[ "$decision" == restart ]] || fail "Update cancelled"
require_origin
# The app may disappear immediately after reading the acknowledgement. All
# subsequent output goes to the log and the copied script holds no old bundle.
trap '' HUP
exec 3>&1 </dev/null >&4 2>&1 4>&-
echo "POKARAK_UPDATE_RESTARTING" >&3
exec 3>&-
ACKNOWLEDGED=true
echo "[$(/bin/date)] Restarting $APP_PATH"
deadline=$((SECONDS + 30))
while /bin/kill -0 "$WAIT_PID" 2>/dev/null; do
    state="$(/bin/ps -o stat= -p "$WAIT_PID" 2>/dev/null)" || break
    [[ "$state" != *Z* ]] || break
    (( SECONDS < deadline )) || fail "Timed out waiting for Pokarak to quit. The installed app was preserved."
    /bin/sleep 0.2
done
[[ "$(bundle_identity "$APP_PATH")" == "$ORIGINAL_ID" ]] || fail "The destination changed while waiting for Pokarak to quit"
users="$(bundle_users "$APP_PATH")" || fail "Cannot verify whether Pokarak has quit"
[[ -z "$users" ]] || fail "Another instance opened $APP_PATH. The installed app was preserved."
[[ "$(bundle_identity "$STAGED_APP")" == "$STAGED_ID" ]] || fail "The staged update changed during restart"
swap_bundles "$APP_PATH" "$STAGED_APP" || fail "Could not replace $APP_PATH"
users="$(bundle_users "$STAGED_APP")" || fail "Cannot verify whether another instance opened the previous bundle"
[[ -z "$users" ]] || fail "Another instance opened the previous Pokarak during replacement"
users="$(bundle_users "$APP_PATH")" || fail "Cannot verify whether the updated app is already running"
if [[ -z "$users" ]]; then
    reopen_app || fail "LaunchServices could not reopen the update"
fi
COMPLETED=true
echo "Updated $APP_PATH"
