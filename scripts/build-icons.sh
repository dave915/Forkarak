#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Default is read-only. Never silently replace the new layered icon with the old
# standalone SVG. Recompilation/import requires explicit --write (see --help).
exec python3 "$ROOT_DIR/scripts/build-liquid-icons.py" "$@"
