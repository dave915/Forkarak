#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$(uname -s)" != Darwin ]]; then
    echo "Icon generation requires macOS, AppKit, and Xcode command-line tools." >&2
    exit 1
fi

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pokarak-icons.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT
ICONSET="$WORK_DIR/Pokarak.iconset"

/usr/bin/xcrun swift "$ROOT_DIR/scripts/render-icon.swift" \
    "$ROOT_DIR/assets/icons/app-icon.svg" "$ICONSET"
/usr/bin/iconutil --convert icns --output "$WORK_DIR/Pokarak.icns" "$ICONSET"
/bin/cp "$WORK_DIR/Pokarak.icns" "$ROOT_DIR/assets/icons/Pokarak.icns"
/bin/cp "$ICONSET/icon_512x512@2x.png" "$ROOT_DIR/assets/icons/app-icon-1024.png"
echo "Generated assets/icons/Pokarak.icns and app-icon-1024.png"
