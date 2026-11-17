#!/bin/bash
# Build an Apple Silicon executable and assemble a signed app bundle.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="${HOME}/.cargo/bin:${PATH}"
# Extract the quoted workspace version without requiring Cargo for --binary.
VERSION="$(/usr/bin/sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\([^"]*\)".*/\1/p' "$ROOT_DIR/Cargo.toml")"
TARGET="aarch64-apple-darwin"
PROFILE="release"
RUN_APP=false
BINARY=""

usage() {
    cat <<'USAGE'
Usage: ./scripts/package-macos.sh [--debug] [--run] [--binary PATH]

  (default)      cargo build --release for aarch64-apple-darwin, then package
  --debug        Build the debug profile instead
  --run          Open dist/Pokarak.app after packaging
  --binary PATH  Package an existing arm64 executable; skip all Cargo commands
  -h, --help     Show this help

Output: dist/Pokarak.app (dev.pokarak.app, version from Cargo.toml)
Honors CARGO_TARGET_DIR. Ad-hoc signing by default. Set SIGNING_IDENTITY to a
Developer ID Application identity for hardened-runtime signing with timestamp.
USAGE
}

fail() { echo "Error: $*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug) PROFILE="debug"; shift ;;
        --run) RUN_APP=true; shift ;;
        --binary)
            [[ $# -ge 2 && -n "$2" && "$2" != --* ]] || fail "--binary requires a path"
            [[ -z "$BINARY" ]] || fail "--binary may be specified only once"
            BINARY="$2"
            shift 2
            ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; fail "Unknown argument: $1" ;;
    esac
done

[[ "$(uname -s)" == Darwin ]] || fail "macOS is required"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "Invalid workspace version: $VERSION"
[[ -z "$BINARY" || "$PROFILE" == release ]] || fail "--debug cannot be combined with --binary"
PLIST="$ROOT_DIR/assets/macos/Info.plist"
ICON="$ROOT_DIR/assets/icons/Pokarak.icns"
[[ -f "$ICON" ]] || fail "Missing icon; run ./scripts/build-icons.sh"
/usr/bin/plutil -lint "$PLIST"

if [[ -z "$BINARY" ]]; then
    command -v cargo >/dev/null 2>&1 || fail "Cargo not found; add \$HOME/.cargo/bin to PATH"
    # Keep the binary deployment target consistent with the bundle declaration.
    export MACOSX_DEPLOYMENT_TARGET=13.0
    BUILD_ARGS=(build --locked --manifest-path "$ROOT_DIR/Cargo.toml" --package pokarak --bin pokarak --target "$TARGET")
    if [[ "$PROFILE" == release ]]; then BUILD_ARGS+=(--release); fi
    cargo "${BUILD_ARGS[@]}"
    METADATA="$(cargo metadata --manifest-path "$ROOT_DIR/Cargo.toml" --format-version 1 --no-deps)"
    TARGET_DIR="$(/usr/bin/plutil -extract target_directory raw -o - - <<< "$METADATA")"
    BINARY="$TARGET_DIR/$TARGET/$PROFILE/pokarak"
fi

# Resolve a relative input before passing it to system tools (including -names).
case "$BINARY" in
    /*) ;;
    *) BINARY="$PWD/$BINARY" ;;
esac
[[ -f "$BINARY" && -x "$BINARY" ]] || fail "Executable not found: $BINARY"
ARCHS="$(/usr/bin/lipo -archs "$BINARY")"
[[ "$ARCHS" == arm64 ]] || fail "Expected an arm64-only executable, found: $ARCHS"

DIST_DIR="$ROOT_DIR/dist"
APP_PATH="$DIST_DIR/Pokarak.app"
mkdir -p "$DIST_DIR"
[[ ! -L "$APP_PATH" ]] || fail "Refusing to replace a symlink: $APP_PATH"
if [[ -e "$APP_PATH" ]]; then
    EXISTING_ID="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$APP_PATH/Contents/Info.plist")"
    [[ "$EXISTING_ID" == dev.pokarak.app ]] || fail "Destination is not a Pokarak bundle: $APP_PATH"
fi

WORK_DIR="$(mktemp -d "$DIST_DIR/.pokarak-package.XXXXXX")"
cleanup() {
    # Restore the previous bundle if publication was interrupted between moves.
    if [[ -d "$WORK_DIR/previous.app" && ! -e "$APP_PATH" ]]; then
        /bin/mv "$WORK_DIR/previous.app" "$APP_PATH" || {
            echo "Previous bundle preserved at $WORK_DIR/previous.app" >&2
            return 1
        }
    fi
    /bin/rm -r "$WORK_DIR"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
STAGED_APP="$WORK_DIR/Pokarak.app"
CONTENTS="$STAGED_APP/Contents"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"
/bin/cp "$BINARY" "$CONTENTS/MacOS/pokarak"
/bin/chmod 755 "$CONTENTS/MacOS/pokarak"
/bin/cp "$PLIST" "$CONTENTS/Info.plist"
/usr/bin/plutil -replace CFBundleShortVersionString -string "$VERSION" "$CONTENTS/Info.plist"
/usr/bin/plutil -replace CFBundleVersion -string "$VERSION" "$CONTENTS/Info.plist"
/bin/cp "$ICON" "$CONTENTS/Resources/Pokarak.icns"
/bin/cp "$ROOT_DIR/assets/icons/spork.svg" "$CONTENTS/Resources/spork.svg"
/bin/cp "$ROOT_DIR/LICENSE" "$CONTENTS/Resources/LICENSE"
mkdir -p "$CONTENTS/Resources/vscode-icons"
/bin/cp "$ROOT_DIR/assets/icons/vscode/NOTICE.md" "$ROOT_DIR/assets/icons/vscode/LICENSE.txt" "$CONTENTS/Resources/vscode-icons/"
/usr/bin/ditto "$ROOT_DIR/assets/fonts" "$CONTENTS/Resources/fonts"
/bin/cp "$ROOT_DIR/scripts/install.sh" "$CONTENTS/Resources/install.sh"
/bin/cp "$ROOT_DIR/scripts/pokarak-cli.sh" "$CONTENTS/Resources/pokarak-cli.sh"
printf 'APPL????' > "$CONTENTS/PkgInfo"

if [[ -n "${SIGNING_IDENTITY:-}" ]]; then
    /usr/bin/codesign --force --options runtime --timestamp --sign "$SIGNING_IDENTITY" "$STAGED_APP"
else
    echo 'Signing ad hoc: this is not Apple Developer ID signing or notarization.'
    /usr/bin/codesign --force --sign - "$STAGED_APP"
fi
/usr/bin/codesign --verify --strict "$STAGED_APP"
if [[ -e "$APP_PATH" ]]; then /bin/mv "$APP_PATH" "$WORK_DIR/previous.app"; fi
/bin/mv "$STAGED_APP" "$APP_PATH"
echo "Packaged: $APP_PATH"
if "$RUN_APP"; then /usr/bin/open "$APP_PATH"; fi
