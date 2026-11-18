# Forkarak artwork

`icons/app/liquid/` contains the official Apple Icon Composer sources for the
macOS 26 layered Liquid Glass app icon. One authored four-tine spork and common
material are used across six background colors. Sources, real Apple-rendered
fallbacks, compiler provenance and reproduction commands are documented in
[Application icons](icons/app/README.md).

`icons/Forkarak.icns` preserves the macOS 13+ flattened fallback.
`icons/app-icon-1024.png` is the matching README preview. These raster exports
are distinct from the native layered catalog. `./scripts/build-icons.sh` verifies
assets by default and cannot silently overwrite them with the old SVG.

`icons/spork.svg` and `icons/app-icon.svg` preserve the separate original line
mark. This project's authored artwork and scripts use the root [MIT license](../LICENSE).
The original approved six designs remain in `icons/app/sources/`.

File lists and tabs use [VSCode Icons](icons/vscode/NOTICE.md) artwork with its
tracked notice and license. File names and compound extensions choose the icon,
including light-theme variants.
