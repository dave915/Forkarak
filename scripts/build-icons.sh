#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$(uname -s)" != Darwin ]]; then
    echo "Icon generation requires macOS, AppKit, and Xcode command-line tools." >&2
    exit 1
fi

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/forkarak-icons.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT
ICONSET="$WORK_DIR/Forkarak.iconset"

/usr/bin/xcrun swift "$ROOT_DIR/scripts/render-icon.swift" \
    "$ROOT_DIR/assets/icons/app-icon.svg" "$ICONSET"
/usr/bin/iconutil --convert icns --output "$WORK_DIR/Forkarak.icns" "$ICONSET"
/bin/cp "$WORK_DIR/Forkarak.icns" "$ROOT_DIR/assets/icons/Forkarak.icns"
/bin/cp "$ICONSET/icon_512x512@2x.png" "$ROOT_DIR/assets/icons/app-icon-1024.png"
echo "Generated assets/icons/Forkarak.icns and app-icon-1024.png"
