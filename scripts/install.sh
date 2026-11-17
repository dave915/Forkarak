#!/bin/bash
# Public release installer. Runs on macOS's system Bash without developer tools.
set -euo pipefail
REPOSITORY="dave915/pokarak"
ASSET="pokarak-macos-arm64.tar.gz"
APP_NAME="Pokarak.app"
ARCHIVE_NAME="포카락.app"
# APFS/HFS+ directory enumeration commonly returns decomposed Korean names.
ARCHIVE_NAME_NFD="포카락.app"
APP_DIR="${HOME}/Applications"
BIN_DIR="${HOME}/.local/bin"
TAG=""
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

Installs the latest public dave915/pokarak release for Apple Silicon macOS 13+.
Defaults: ~/Applications/Pokarak.app and ~/.local/bin/pokarak.
Use `pokarak update` to update the same installation. Quit the app first.
No sudo, Rust, Xcode, Homebrew, or GitHub login is needed.
USAGE
}
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version|--app-dir|--bin-dir)
            [[ $# -ge 2 && -n "$2" && "$2" != --* ]] || fail "$1 requires a value"
            case "$1" in
                --version) [[ -z "$TAG" ]] || fail "--version specified twice"; TAG="$2" ;;
                --app-dir) APP_DIR="$2" ;;
                --bin-dir) BIN_DIR="$2" ;;
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
case "$APP_DIR" in /*) ;; *) APP_DIR="$PWD/$APP_DIR" ;; esac
case "$BIN_DIR" in /*) ;; *) BIN_DIR="$PWD/$BIN_DIR" ;; esac
[[ "$APP_DIR" != *$'\n'* && "$BIN_DIR" != *$'\n'* ]] || fail "Installation directories cannot contain newlines"
mkdir -p "$APP_DIR" "$BIN_DIR"
APP_DIR="$(cd "$APP_DIR" && pwd -P)"
BIN_DIR="$(cd "$BIN_DIR" && pwd -P)"
APP_PATH="$APP_DIR/$APP_NAME"
LEGACY_PATH="$APP_DIR/$ARCHIVE_NAME"
CLI_PATH="$BIN_DIR/pokarak"
LOCK_DIR="$APP_DIR/.pokarak-install.lock"
mkdir "$LOCK_DIR" 2>/dev/null || fail "Another install is active, or a stale lock exists at $LOCK_DIR"

cleanup() {
    status=$?
    if ! "$COMMITTED" && [[ -n "$WORK_DIR" ]]; then
        if "$APP_PUBLISHED" && [[ -e "$APP_PATH" ]]; then /bin/rm -rf "$APP_PATH"; fi
        if [[ -d "$WORK_DIR/previous.app" ]]; then
            /bin/mv "$WORK_DIR/previous.app" "$APP_PATH" || { echo "Previous app preserved at $WORK_DIR/previous.app" >&2; return 1; }
        fi
        if [[ -d "$WORK_DIR/legacy.app" ]]; then
            /bin/mv "$WORK_DIR/legacy.app" "$LEGACY_PATH" || { echo "Previous app preserved at $WORK_DIR/legacy.app" >&2; return 1; }
        fi
        if [[ -n "$CLI_BACKUP" && -e "$CLI_BACKUP" ]]; then
            /bin/mv -f "$CLI_BACKUP" "$CLI_PATH" || { echo "Previous CLI preserved at $CLI_BACKUP" >&2; return 1; }
        fi
    fi
    [[ -z "$APP_STAGE" || ! -e "$APP_STAGE" ]] || /bin/rm -rf "$APP_STAGE"
    [[ -z "$CLI_STAGE" || ! -e "$CLI_STAGE" ]] || /bin/rm -f "$CLI_STAGE"
    [[ -z "$CLI_BACKUP" || ! -e "$CLI_BACKUP" ]] || /bin/rm -f "$CLI_BACKUP"
    [[ -z "$WORK_DIR" || ! -d "$WORK_DIR" ]] || /bin/rm -rf "$WORK_DIR"
    rmdir "$LOCK_DIR" 2>/dev/null || true
    return "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

[[ ! -L "$APP_PATH" ]] || fail "Refusing to replace a symlink: $APP_PATH"
if [[ -e "$APP_PATH" ]]; then
    [[ -d "$APP_PATH" ]] || fail "Destination is not an app: $APP_PATH"
    EXISTING_ID="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$APP_PATH/Contents/Info.plist" 2>/dev/null)" || fail "Destination is not a Pokarak bundle"
    [[ "$EXISTING_ID" == dev.pokarak.app ]] || fail "Destination is not a Pokarak bundle"
fi
[[ ! -L "$CLI_PATH" ]] || fail "Refusing to replace a symlink: $CLI_PATH"
if [[ -e "$CLI_PATH" ]]; then
    [[ -f "$CLI_PATH" ]] && /usr/bin/grep -Fq '# Pokarak managed launcher' "$CLI_PATH" || fail "Refusing to replace an unrelated command: $CLI_PATH"
fi
managed_legacy_app() {
    [[ -d "$LEGACY_PATH" && ! -L "$LEGACY_PATH" ]] &&
        [[ "$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$LEGACY_PATH/Contents/Info.plist" 2>/dev/null)" == dev.pokarak.app ]]
}
require_app_closed() {
    # Match the executable's inode even when the caller used a normalized path.
    if [[ -f "$1/Contents/MacOS/pokarak" ]] && /usr/sbin/lsof -t -a -d txt "$1/Contents/MacOS/pokarak" >/dev/null 2>&1; then
        fail "Quit Pokarak at $1, then retry the update"
    fi
}
require_app_closed "$APP_PATH"
if managed_legacy_app; then require_app_closed "$LEGACY_PATH"; fi
WORK_DIR="$(mktemp -d "$APP_DIR/.pokarak-install.XXXXXX")"
RELEASES_URL="https://github.com/$REPOSITORY/releases"
# Keep diagnostics tied to the public URL; asset redirects can carry a temporary
# signed query string. Successful requests return curl's status and redirect.
release_request() {
    local step="$1" url="$2" response code rc
    shift 2
    if response="$(curl --fail --silent --show-error --proto '=https' --proto-redir '=https' --tlsv1.2 \
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
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "Expected a stable release tag such as v0.1.0, found: $TAG"
RELEASE_URL="$RELEASES_URL/download/$TAG"
echo "Downloading Pokarak ${TAG}..."
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
        "$ARCHIVE_NAME"|"$ARCHIVE_NAME/"|"$ARCHIVE_NAME/"*|"$ARCHIVE_NAME_NFD"|"$ARCHIVE_NAME_NFD/"|"$ARCHIVE_NAME_NFD/"*) ;;
        *) fail "Unexpected archive entry: $entry" ;;
    esac
    case "/${entry%/}/" in *'/../'*|*'/./'*|*'//'*) fail "Unsafe archive entry: $entry" ;; esac
done < "$WORK_DIR/entries"
mkdir "$WORK_DIR/unpacked"
/usr/bin/tar -xzf "$WORK_DIR/$ASSET" -C "$WORK_DIR/unpacked"
STAGED_APP="$WORK_DIR/unpacked/$ARCHIVE_NAME"
[[ -d "$STAGED_APP" && ! -L "$STAGED_APP" ]] || fail "Release does not contain an app bundle"
# Our bundles have no symlinks. Refuse any before traversing signed resources.
[[ -z "$(/usr/bin/find "$STAGED_APP" -type l -print -quit)" ]] || fail "Release bundle contains a symlink"
PLIST="$STAGED_APP/Contents/Info.plist"
[[ "$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$PLIST")" == dev.pokarak.app ]] || fail "Unexpected bundle identifier"
[[ "$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$PLIST")" == "${TAG#v}" ]] || fail "Bundle version does not match release tag"
[[ "$(/usr/bin/plutil -extract CFBundleExecutable raw -o - "$PLIST")" == pokarak ]] || fail "Unexpected bundle executable"
[[ -x "$STAGED_APP/Contents/MacOS/pokarak" ]] || fail "Release executable is missing"
# lipo is an Xcode/Command Line Tools shim on clean Macs. file ships with macOS
# and identifies the thin Mach-O executable without invoking developer tools.
[[ "$(LC_ALL=C /usr/bin/file -b "$STAGED_APP/Contents/MacOS/pokarak")" == 'Mach-O 64-bit executable arm64' ]] || fail "Release executable is not an arm64 Mach-O executable"
[[ -f "$STAGED_APP/Contents/Resources/pokarak-cli.sh" && -f "$STAGED_APP/Contents/Resources/install.sh" ]] || fail "Release CLI resources are missing"
/usr/bin/codesign --verify --strict "$STAGED_APP" || fail "App signature verification failed"

APP_STAGE="$STAGED_APP"
CLI_STAGE="$(mktemp "$BIN_DIR/.pokarak-cli.XXXXXX")"
{
    printf '#!/bin/bash\n# Pokarak managed launcher\n'
    printf 'POKARAK_APP_PATH=%s\n' "$(shell_quote "$APP_PATH")"
    printf 'POKARAK_BIN_DIR=%s\n' "$(shell_quote "$BIN_DIR")"
    /bin/cat "$APP_STAGE/Contents/Resources/pokarak-cli.sh"
} > "$CLI_STAGE"
/bin/chmod 755 "$CLI_STAGE"
require_app_closed "$APP_PATH"
if managed_legacy_app; then
    require_app_closed "$LEGACY_PATH"
    /bin/mv "$LEGACY_PATH" "$WORK_DIR/legacy.app"
fi
if [[ -e "$APP_PATH" ]]; then /bin/mv "$APP_PATH" "$WORK_DIR/previous.app"; fi
if [[ -e "$CLI_PATH" ]]; then
    CLI_BACKUP="$(mktemp "$BIN_DIR/.pokarak-previous.XXXXXX")"
    /bin/rm "$CLI_BACKUP"
    /bin/mv "$CLI_PATH" "$CLI_BACKUP"
fi
/bin/mv "$APP_STAGE" "$APP_PATH"
APP_PUBLISHED=true
/bin/mv -f "$CLI_STAGE" "$CLI_PATH"
COMMITTED=true
echo "Installed Pokarak ${TAG#v}: $APP_PATH"
echo "Command: $CLI_PATH"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) printf 'Add the command to this shell: export PATH=%s:"$PATH"\n' "$(shell_quote "$BIN_DIR")" ;;
esac
echo 'Update after quitting the app: pokarak update'
echo 'This build may be ad-hoc signed; Apple Developer ID signing and notarization are separate.'
