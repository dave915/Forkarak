#!/bin/bash
# Public release installer. Runs on macOS's system Bash without developer tools.
set -euo pipefail
REPOSITORY="dave915/Forkarak"
ASSET="forkarak-macos-arm64.tar.gz"
APP_NAME="Forkarak.app"
APP_DIR=""
BIN_DIR=""
TAG=""
CHECK_ONLY=false
INSTALL_DIR_GIVEN=false
RUNNING_APP=""
WAIT_PID=""
SESSION_DIR=""
WORK_DIR=""
LOCK_DIR=""
APP_STAGE=""
CLI_STAGE=""
APP_PATH=""
CLI_PATH=""
CLI_BACKUP=""
COMMITTED=false
APP_PUBLISHED=false

fail() { echo "Error: $*" >&2; exit 1; }
# Bash 3.2 must update its locale before %q; an inline environment assignment
# leaves partially escaped multi-byte strings in some locale configurations.
shell_quote() { (LC_ALL=C; printf '%q' "$1"); }
usage() {
    cat <<'USAGE'
Usage: bash install.sh [--version vX.Y.Z] [--app-dir DIRECTORY] [--bin-dir DIRECTORY]
       bash install.sh --check-only
       bash install.sh --update-running-app APP_BUNDLE --wait-pid PID --version vX.Y.Z
                       [--relaunch-session-dir ABSOLUTE_DIRECTORY]

Installs the latest public dave915/Forkarak release for Apple Silicon macOS 13+.
Defaults: ~/Applications/Forkarak.app and ~/.local/bin/forkarak.
Use `forkarak update` to update the same installation. Quit the app first.
No sudo, Rust, Xcode, Homebrew, or GitHub login is needed.
USAGE
}
if [[ "${1:-}" == --check-only ]]; then
    [[ $# -eq 1 ]] || fail "--check-only must be used alone"
    CHECK_ONLY=true
    shift
fi
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version|--app-dir|--bin-dir|--update-running-app|--wait-pid|--relaunch-session-dir)
            [[ $# -ge 2 && -n "$2" && "$2" != --* ]] || fail "$1 requires a value"
            case "$1" in
                --version) [[ -z "$TAG" ]] || fail "--version specified twice"; TAG="$2" ;;
                --app-dir) APP_DIR="$2"; INSTALL_DIR_GIVEN=true ;;
                --bin-dir) BIN_DIR="$2"; INSTALL_DIR_GIVEN=true ;;
                --update-running-app) [[ -z "$RUNNING_APP" ]] || fail "$1 specified twice"; RUNNING_APP="$2" ;;
                --wait-pid) [[ -z "$WAIT_PID" ]] || fail "$1 specified twice"; WAIT_PID="$2" ;;
                --relaunch-session-dir) [[ -z "$SESSION_DIR" ]] || fail "$1 specified twice"; SESSION_DIR="$2" ;;
            esac
            shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) fail "Unknown argument: $1" ;;
    esac
done
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || fail "Apple Silicon macOS is required"
OS_VERSION="$(/usr/bin/sw_vers -productVersion)"
[[ "${OS_VERSION%%.*}" -ge 13 ]] || fail "macOS 13 or newer is required"
command -v curl >/dev/null 2>&1 || fail "curl is required"
RELEASES_URL="https://github.com/$REPOSITORY/releases"
# Keep diagnostics tied to the public URL; asset redirects can carry a temporary
# signed query string. Successful requests return curl's status and redirect.
release_request() {
    local step="$1" url="$2" response code rc
    shift 2
    if response="$(curl -q --fail --silent --show-error --proto '=https' --proto-redir '=https' --tlsv1.2 \
            --connect-timeout 15 --max-time 600 --retry 2 "$@" --write-out '%{http_code} %{redirect_url}' --url "$url")"; then
        printf '%s' "$response"
    else
        rc=$?
        code="${response%% *}"
        echo "Error: $step failed at $url (HTTP ${code:-000}, curl $rc)." >&2
        if [[ "$code" == 403 ]]; then
            echo "GitHub or your network denied access. Check whether you can open this public release URL in your browser." >&2
        fi
        if [[ "$url" == "$RELEASES_URL/latest" ]]; then
            echo "To skip latest-version lookup, rerun with --version vX.Y.Z for a published tag." >&2
        fi
        return "$rc"
    fi
}

resolve_tag() {
    local latest status location
    if [[ -z "$TAG" ]]; then
        # GitHub redirects this public web URL directly to the latest release tag.
        # Read only that first redirect and pin every asset to the same exact tag.
        latest="$(release_request "Latest release lookup" "$RELEASES_URL/latest" --head --no-location --output /dev/null)"
        status="${latest%% *}"
        location="${latest#* }"
        case "$status" in
            301|302|303|307|308) ;;
            *) fail "Latest release lookup at $RELEASES_URL/latest returned HTTP $status without a release redirect" ;;
        esac
        [[ "$location" == "$RELEASES_URL/tag/"* ]] || fail "Unexpected redirect from $RELEASES_URL/latest; expected a tag in $REPOSITORY"
        TAG="${location#"$RELEASES_URL/tag/"}"
    fi
    [[ "$TAG" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || fail "Expected a stable release tag such as v0.1.0, found: $TAG"
}

if "$CHECK_ONLY"; then
    resolve_tag
    printf '%s\n' "$TAG"
    exit 0
fi

[[ -n "${HOME:-}" ]] || fail "Cannot locate the home directory for installation"
if [[ -n "$RUNNING_APP" ]]; then
    ! "$INSTALL_DIR_GIVEN" || fail "App updates cannot change installation directories"
    [[ -n "$TAG" && "$WAIT_PID" =~ ^[1-9][0-9]*$ ]] || fail "A running app update requires --version vX.Y.Z and --wait-pid PID"
    resolve_tag
    [[ "$RUNNING_APP" == /*.app && "$RUNNING_APP" != *$'\n'* ]] || fail "Expected an absolute app bundle path"
    [[ -z "$SESSION_DIR" || "$SESSION_DIR" == /* ]] || fail "Expected an absolute session directory"
    APP_DIR="$(cd -P -- "${RUNNING_APP%/*}/" && pwd -P)" || fail "Cannot access the app's parent directory"
    APP_PATH="${APP_DIR%/}/${RUNNING_APP##*/}"
    [[ -d "$APP_PATH" && -w "$APP_DIR" ]] || fail "Move Forkarak into a writable application directory and try again"
    HELPER="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)/update-handoff.sh"
    [[ -f "$HELPER" && ! -L "$HELPER" ]] || fail "The installed update helper is missing"
    source "$HELPER"
else
    [[ -z "$WAIT_PID" && -z "$SESSION_DIR" ]] || fail "--wait-pid and --relaunch-session-dir require --update-running-app"
    APP_DIR="${APP_DIR:-$HOME/Applications}"
    BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
    case "$APP_DIR" in /*) ;; *) APP_DIR="$PWD/$APP_DIR" ;; esac
    case "$BIN_DIR" in /*) ;; *) BIN_DIR="$PWD/$BIN_DIR" ;; esac
    [[ "$APP_DIR" != *$'\n'* && "$BIN_DIR" != *$'\n'* ]] || fail "Installation directories cannot contain newlines"
    mkdir -p "$APP_DIR" "$BIN_DIR"
    APP_DIR="$(cd "$APP_DIR" && pwd -P)"
    BIN_DIR="$(cd "$BIN_DIR" && pwd -P)"
    APP_PATH="$APP_DIR/$APP_NAME"
    CLI_PATH="$BIN_DIR/forkarak"
fi
LOCK_DIR="$APP_DIR/.forkarak-install.lock"
mkdir "$LOCK_DIR" 2>/dev/null || fail "Another install is active, or a stale lock exists at $LOCK_DIR"

cleanup() {
    status=$?
    if ! "$COMMITTED" && [[ -n "$WORK_DIR" ]]; then
        if "$APP_PUBLISHED" && [[ -e "$APP_PATH" ]]; then /bin/rm -rf "$APP_PATH"; fi
        if [[ -d "$WORK_DIR/previous.app" ]]; then
            /bin/mv "$WORK_DIR/previous.app" "$APP_PATH" || { echo "Previous app preserved at $WORK_DIR/previous.app" >&2; return 1; }
        fi
        if [[ -n "$CLI_BACKUP" && -e "$CLI_BACKUP" ]]; then
            /bin/mv -f "$CLI_BACKUP" "$CLI_PATH" || { echo "Previous CLI preserved at $CLI_BACKUP" >&2; return 1; }
        fi
    fi
    [[ -z "$APP_STAGE" || ! -e "$APP_STAGE" ]] || /bin/rm -rf "$APP_STAGE"
    [[ -z "$CLI_STAGE" || ! -e "$CLI_STAGE" ]] || /bin/rm -f "$CLI_STAGE"
    [[ -z "$CLI_BACKUP" || ! -e "$CLI_BACKUP" ]] || /bin/rm -f "$CLI_BACKUP"
    [[ -z "$WORK_DIR" || ! -d "$WORK_DIR" ]] || /bin/rm -rf "$WORK_DIR"
    [[ -z "$RUNNING_APP" ]] || /bin/rm -f "$LOCK_DIR/work-dir"
    rmdir "$LOCK_DIR" 2>/dev/null || true
    return "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 141' PIPE

[[ ! -L "$APP_PATH" ]] || fail "Refusing to replace a symlink: $APP_PATH"
if [[ -e "$APP_PATH" ]]; then
    [[ -d "$APP_PATH" ]] || fail "Destination is not an app: $APP_PATH"
    EXISTING_ID="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$APP_PATH/Contents/Info.plist" 2>/dev/null)" || fail "Destination is not a Forkarak bundle"
    EXISTING_EXECUTABLE="$(/usr/bin/plutil -extract CFBundleExecutable raw -o - "$APP_PATH/Contents/Info.plist" 2>/dev/null)" || fail "Destination is not a Forkarak bundle"
    [[ "$EXISTING_ID" == dev.pokarak.app && "$EXISTING_EXECUTABLE" == forkarak ]] || fail "Destination is not a Forkarak bundle"
fi
require_app_closed() {
    # Match the executable's inode even when the caller used a normalized path.
    if [[ -f "$1/Contents/MacOS/forkarak" ]] && /usr/sbin/lsof -t -a -d txt "$1/Contents/MacOS/forkarak" >/dev/null 2>&1; then
        fail "Quit Forkarak at $1, then retry the update"
    fi
}
if [[ -n "$RUNNING_APP" ]]; then
    [[ ! -L "$APP_PATH/Contents" && ! -L "$APP_PATH/Contents/Info.plist" && ! -L "$APP_PATH/Contents/MacOS" ]] ||
        fail "The installed bundle contains an unexpected symlink"
    EXECUTABLE="$APP_PATH/Contents/MacOS/forkarak"
    [[ -f "$EXECUTABLE" && -x "$EXECUTABLE" && ! -L "$EXECUTABLE" ]] || fail "Unexpected installed bundle executable"
    ORIGINAL_ID="$(bundle_identity "$APP_PATH")"
    require_origin
else
    [[ ! -L "$CLI_PATH" ]] || fail "Refusing to replace a symlink: $CLI_PATH"
    if [[ -e "$CLI_PATH" ]]; then
        [[ -f "$CLI_PATH" ]] && /usr/bin/grep -Fq '# Forkarak managed launcher' "$CLI_PATH" || fail "Refusing to replace an unrelated command: $CLI_PATH"
    fi
    require_app_closed "$APP_PATH"
    resolve_tag
fi
WORK_DIR="$(mktemp -d "$APP_DIR/.forkarak-install.XXXXXX")"
RELEASE_URL="$RELEASES_URL/download/$TAG"
echo "Downloading Forkarak ${TAG}..."
release_request "App archive download" "$RELEASE_URL/$ASSET" --location --output "$WORK_DIR/$ASSET" >/dev/null
release_request "Checksum download" "$RELEASE_URL/SHA256SUMS" --location --output "$WORK_DIR/SHA256SUMS" >/dev/null
EXPECTED="$(/usr/bin/awk -v name="$ASSET" '$2 == name { print $1 }' "$WORK_DIR/SHA256SUMS")"
[[ "$EXPECTED" =~ ^[[:xdigit:]]{64}$ ]] || fail "Missing or invalid archive checksum"
ACTUAL="$(/usr/bin/shasum -a 256 "$WORK_DIR/$ASSET")"
[[ "${ACTUAL%% *}" == "$EXPECTED" ]] || fail "Archive checksum verification failed; existing installation was preserved"

# Reject absolute/traversing paths and extra top-level entries before unpacking.
/usr/bin/tar -tzf "$WORK_DIR/$ASSET" > "$WORK_DIR/entries"
/usr/bin/tar -tvzf "$WORK_DIR/$ASSET" > "$WORK_DIR/types"
/usr/bin/awk 'substr($0,1,1) != "-" && substr($0,1,1) != "d" { exit 1 }' "$WORK_DIR/types" || fail "Archive contains links or special files"
while IFS= read -r entry; do
    case "$entry" in
        "$APP_NAME"|"$APP_NAME/"*) ;;
        *) fail "Unexpected archive entry: $entry" ;;
    esac
    case "/${entry%/}/" in *'/../'*|*'/./'*|*'//'*) fail "Unsafe archive entry: $entry" ;; esac
done < "$WORK_DIR/entries"
mkdir "$WORK_DIR/unpacked"
/usr/bin/tar -xzf "$WORK_DIR/$ASSET" -C "$WORK_DIR/unpacked"
STAGED_APP="$WORK_DIR/unpacked/$APP_NAME"
[[ -d "$STAGED_APP" && ! -L "$STAGED_APP" ]] || fail "Release does not contain an app bundle"
# Our bundles have no symlinks. Refuse any before traversing signed resources.
[[ -z "$(/usr/bin/find "$STAGED_APP" -type l -print -quit)" ]] || fail "Release bundle contains a symlink"
PLIST="$STAGED_APP/Contents/Info.plist"
[[ "$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$PLIST")" == dev.pokarak.app ]] || fail "Unexpected bundle identifier"
[[ "$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$PLIST")" == "${TAG#v}" ]] || fail "Bundle version does not match release tag"
[[ "$(/usr/bin/plutil -extract CFBundleExecutable raw -o - "$PLIST")" == forkarak ]] || fail "Unexpected bundle executable"
[[ -x "$STAGED_APP/Contents/MacOS/forkarak" ]] || fail "Release executable is missing"
# lipo is an Xcode/Command Line Tools shim on clean Macs. file ships with macOS
# and identifies the thin Mach-O executable without invoking developer tools.
[[ "$(LC_ALL=C /usr/bin/file -b "$STAGED_APP/Contents/MacOS/forkarak")" == 'Mach-O 64-bit executable arm64' ]] || fail "Release executable is not an arm64 Mach-O executable"
[[ -f "$STAGED_APP/Contents/Resources/forkarak-cli.sh" && -f "$STAGED_APP/Contents/Resources/install.sh" ]] || fail "Release CLI resources are missing"
/usr/bin/codesign --verify --strict --deep "$STAGED_APP" || fail "App signature verification failed"
# Older public bundles predate managed language resources. When either new
# resource is present, require the complete pair and verify without devtools.
LANGUAGE_ROOT="$STAGED_APP/Contents/Resources/language-tools"
LANGUAGE_CHECK="$STAGED_APP/Contents/Resources/verify-language-resources.sh"
if [[ -e "$LANGUAGE_ROOT" || -e "$LANGUAGE_CHECK" ]]; then
    [[ -d "$LANGUAGE_ROOT" && -f "$LANGUAGE_CHECK" ]] || fail 'Language resource validation files are missing'
    /bin/bash "$LANGUAGE_CHECK" "$LANGUAGE_ROOT" || fail 'Bundled language resource validation failed'
fi

if [[ -n "$RUNNING_APP" ]]; then
    [[ "$(/usr/bin/plutil -extract CFBundlePackageType raw -o - "$PLIST")" == APPL ]] || fail "Release is not a launchable application"
    if minimum="$(/usr/bin/plutil -extract LSMinimumSystemVersion raw -o - "$PLIST" 2>/dev/null)"; then
        [[ "$minimum" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]] || fail "Invalid minimum macOS version"
        /usr/bin/awk -v os="$OS_VERSION" -v minimum="$minimum" 'BEGIN {
            split(os, current, "."); split(minimum, required, ".")
            for (i = 1; i <= 3; i++) {
                if (current[i]+0 != required[i]+0) exit (current[i]+0 < required[i]+0)
            }
        }' || fail "This release requires macOS $minimum or newer"
    fi
    # Re-exec outside the installed bundle before asking the GUI to restart.
    /bin/cp "$HELPER" "$WORK_DIR/update-handoff.sh"
    /bin/bash -n "$WORK_DIR/update-handoff.sh"
    printf '%s\n' "$WORK_DIR" > "$LOCK_DIR/work-dir"
    cd /
    exec /bin/bash "$WORK_DIR/update-handoff.sh" "$APP_PATH" "$WAIT_PID" "$ORIGINAL_ID" "$SESSION_DIR"
fi

APP_STAGE="$STAGED_APP"
CLI_STAGE="$(mktemp "$BIN_DIR/.forkarak-cli.XXXXXX")"
{
    printf '#!/bin/bash\n# Forkarak managed launcher\n'
    printf 'FORKARAK_APP_PATH=%s\n' "$(shell_quote "$APP_PATH")"
    printf 'FORKARAK_BIN_DIR=%s\n' "$(shell_quote "$BIN_DIR")"
    /bin/cat "$APP_STAGE/Contents/Resources/forkarak-cli.sh"
} > "$CLI_STAGE"
/bin/chmod 755 "$CLI_STAGE"
require_app_closed "$APP_PATH"
if [[ -e "$APP_PATH" ]]; then /bin/mv "$APP_PATH" "$WORK_DIR/previous.app"; fi
if [[ -e "$CLI_PATH" ]]; then
    CLI_BACKUP="$(mktemp "$BIN_DIR/.forkarak-previous.XXXXXX")"
    /bin/rm "$CLI_BACKUP"
    /bin/mv "$CLI_PATH" "$CLI_BACKUP"
fi
/bin/mv "$APP_STAGE" "$APP_PATH"
APP_PUBLISHED=true
/bin/mv -f "$CLI_STAGE" "$CLI_PATH"
COMMITTED=true
echo "Installed Forkarak ${TAG#v}: $APP_PATH"
echo "Command: $CLI_PATH"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) printf 'Add the command to this shell: export PATH=%s:"$PATH"\n' "$(shell_quote "$BIN_DIR")" ;;
esac
echo 'Update after quitting the app: forkarak update'
echo 'This build may be ad-hoc signed; Apple Developer ID signing and notarization are separate.'
