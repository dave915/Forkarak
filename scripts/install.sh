#!/bin/bash
# Public release installer. Runs on macOS's system Bash without developer tools.
set -euo pipefail
REPOSITORY="dave915/pokarak"
ASSET="pokarak-macos-arm64.tar.gz"
APP_NAME="포카락.app"
# APFS/HFS+ directory enumeration commonly returns decomposed Korean names.
APP_NAME_NFD="포카락.app"
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
Defaults: ~/Applications/포카락.app and ~/.local/bin/pokarak.
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
app_running() {
    # Match the executable's inode rather than its spelling. ps can retain a
    # /var alias while pwd returns /private/var, and macOS normalizes Unicode.
    [[ -f "$APP_PATH/Contents/MacOS/pokarak" ]] &&
        /usr/sbin/lsof -t -a -d txt "$APP_PATH/Contents/MacOS/pokarak" >/dev/null 2>&1
}
app_running && fail "Quit Pokarak at $APP_PATH, then retry the update"
WORK_DIR="$(mktemp -d "$APP_DIR/.pokarak-install.XXXXXX")"
download() { curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 --connect-timeout 15 --max-time 600 --retry 2 "$1" --output "$2"; }
if [[ -z "$TAG" ]]; then
    download "https://api.github.com/repos/$REPOSITORY/releases/latest" "$WORK_DIR/release.json"
    TAG="$(/usr/bin/plutil -extract tag_name raw -o - "$WORK_DIR/release.json")" || fail "Could not determine the latest release"
fi
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "Expected a stable release tag such as v0.1.0, found: $TAG"
RELEASE_URL="https://github.com/$REPOSITORY/releases/download/$TAG"
echo "Downloading Pokarak ${TAG}..."
download "$RELEASE_URL/$ASSET" "$WORK_DIR/$ASSET"
download "$RELEASE_URL/SHA256SUMS" "$WORK_DIR/SHA256SUMS"
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
        "$APP_NAME"|"$APP_NAME/"|"$APP_NAME/"*|"$APP_NAME_NFD"|"$APP_NAME_NFD/"|"$APP_NAME_NFD/"*) ;;
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
[[ "$(/usr/bin/plutil -extract CFBundleExecutable raw -o - "$PLIST")" == pokarak ]] || fail "Unexpected bundle executable"
[[ -x "$STAGED_APP/Contents/MacOS/pokarak" ]] || fail "Release executable is missing"
[[ "$(/usr/bin/lipo -archs "$STAGED_APP/Contents/MacOS/pokarak")" == arm64 ]] || fail "Release executable is not Apple Silicon"
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
app_running && fail "Quit Pokarak before updating; existing installation was preserved"
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
