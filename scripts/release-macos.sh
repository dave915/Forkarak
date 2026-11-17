#!/bin/bash
# Produce the exact public assets consumed by install.sh.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAG=""
BINARY=""
fail() { echo "Error: $*" >&2; exit 1; }
usage() {
    cat <<'USAGE'
Usage: ./scripts/release-macos.sh --version vX.Y.Z [--binary PATH]

Builds with Cargo --locked (or packages an existing arm64 binary) and produces:
  dist/release/Pokarak-macos-arm64.zip
  dist/release/pokarak-macos-arm64.tar.gz
  dist/release/SHA256SUMS
  dist/release/install.sh

The release tag must match workspace.package.version in Cargo.toml.
Optional Developer ID: SIGNING_IDENTITY. Optional Apple notarization:
NOTARY_KEYCHAIN_PROFILE (requires SIGNING_IDENTITY and Xcode notarytool/stapler).
Without credentials the bundle is signed ad hoc, without Apple notarization.
USAGE
}
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version|--binary)
            [[ $# -ge 2 && -n "$2" && "$2" != --* ]] || fail "$1 requires a value"
            case "$1" in
                --version) [[ -z "$TAG" ]] || fail "--version specified twice"; TAG="$2" ;;
                --binary) [[ -z "$BINARY" ]] || fail "--binary specified twice"; BINARY="$2" ;;
            esac
            shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) fail "Unknown argument: $1" ;;
    esac
done
VERSION="$(/usr/bin/sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\([^"]*\)".*/\1/p' "$ROOT_DIR/Cargo.toml")"
[[ "$TAG" == "v$VERSION" && "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "--version must match workspace version v$VERSION"
if [[ -n "${NOTARY_KEYCHAIN_PROFILE:-}" && -z "${SIGNING_IDENTITY:-}" ]]; then
    fail "Notarization requires SIGNING_IDENTITY"
fi
# System Bash 3.2 treats an empty array expansion as unset under nounset.
if [[ -n "$BINARY" ]]; then
    "$ROOT_DIR/scripts/package-macos.sh" --binary "$BINARY"
else
    "$ROOT_DIR/scripts/package-macos.sh"
fi
APP_PATH="$ROOT_DIR/dist/Pokarak.app"
STAGE="$(mktemp -d "$ROOT_DIR/dist/.pokarak-release.XXXXXX")"
cleanup() { /bin/rm -rf "$STAGE"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
if [[ -n "${NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
    /usr/bin/ditto -c -k --keepParent "$APP_PATH" "$STAGE/notarization.zip"
    /usr/bin/xcrun notarytool submit "$STAGE/notarization.zip" --keychain-profile "$NOTARY_KEYCHAIN_PROFILE" --wait
    /usr/bin/xcrun stapler staple "$APP_PATH"
    /usr/bin/xcrun stapler validate "$APP_PATH"
else
    echo 'Apple notarization is not configured; release is not notarized.'
fi
/usr/bin/codesign --verify --strict "$APP_PATH"
# Do not ship Finder metadata or quarantine attributes from the build machine.
/usr/bin/ditto -c -k --norsrc --keepParent "$APP_PATH" "$STAGE/Pokarak-macos-arm64.zip"
# Installers shipped through v0.2.0 only accept the original archive root.
COPYFILE_DISABLE=1 /usr/bin/tar -czf "$STAGE/pokarak-macos-arm64.tar.gz" -s '|^Pokarak[.]app|포카락.app|' -C "$ROOT_DIR/dist" Pokarak.app
/bin/cp "$ROOT_DIR/scripts/install.sh" "$STAGE/install.sh"
(
    cd "$STAGE"
    /usr/bin/shasum -a 256 Pokarak-macos-arm64.zip pokarak-macos-arm64.tar.gz install.sh > SHA256SUMS
)
OUTPUT="$ROOT_DIR/dist/release"
[[ ! -L "$OUTPUT" ]] || fail "Refusing a symlink release directory"
mkdir -p "$OUTPUT"
for asset in Pokarak-macos-arm64.zip pokarak-macos-arm64.tar.gz SHA256SUMS install.sh; do
    /bin/mv -f "$STAGE/$asset" "$OUTPUT/$asset"
done
echo "Release assets prepared for $TAG at $OUTPUT"
