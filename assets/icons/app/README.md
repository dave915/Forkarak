# Forkarak application icons

The shipped default is the approved charcoal tile with a silver metal spork:
four short tines, a rounded spoon bowl and a short handle. Presets change the
tile background to charcoal, deep blue, deep green, plum, ivory or light blue.
Existing SVG logos and the `vscode/` file artwork are separate assets.

## Sources and provenance

`sources/selected-reference.jpg` and `sources/selected-original.png` preserve
the selected original, including its photographic backdrop. That backdrop is
**not** included in production icons. `sources/{color}.png` are unmodified
outputs from the coordinator's built-in `image_gen` workflow. Exact generation
prompts and original source locations are preserved in
`sources/imagegen-prompts.json` and `sources/source-manifest.json`.
Those external paths are provenance records; builds use the tracked local copies.

The human approvals are recorded in source thread
`01a1ea1f-8ba9-7d80-a252-2039dd203c02`, turns
`01a1ea35-ea02-7db2-b1c6-4a5c1c8e81bd` and
`01a1ea3a-1021-78c1-b794-5e92eee69f68`.

## Reproduce production formats

On macOS, from any working directory:

```sh
swift /path/to/Forkarak/scripts/generate-app-icons.swift --qa-dir /tmp/forkarak-icon-qa
```

The helper uses CoreGraphics, ImageIO and `/usr/bin/iconutil`; it makes no
semantic edits to the spork. All sources share the original 1254-square canvas.
A common rounded-rectangle coverage mask trims the generated outside islands
and fringe, with source-space bounds x=100..1158, top=114..1172 and radius=244.
It preserves the spork and its shadow and leaves actual transparent pixels
outside the tile. The output canvas is 1024 square. Each smaller representation
is prepared directly from the original source with the same native coverage
mask, avoiding successive downscaling and color-dependent alpha rounding.

Each color has a production PNG, ICNS and complete tracked `.iconset` with
16, 16@2x, 32, 32@2x, 128, 128@2x, 256, 256@2x, 512 and 512@2x representations.
The helper checks common alpha, transparent canvas edges, and all ten ICNS
representations after an `iconutil` round trip. Optional QA sheets show actual
32/64/128-pixel images on light and dark backgrounds, in the preset order above.

`../Forkarak.icns` is an exact copy of `charcoal.icns` and is already consumed
by `CFBundleIconFile` and the existing macOS packaging copy.
`../Forkarak-1024.png` is the corresponding shipped-default preview/export.

## Runtime behavior

The native application embeds preset ICNS data and preview PNG data in its
executable, so Finder launch and a short PATH need no external asset lookup.
The app-only `appearance.app_icon` preference stores semantic preset IDs.
`follow_default` clears `NSApplication.applicationIconImage` with `nil`, allowing
the installed bundle's default to change in a later release. Explicit `charcoal`
remains an override. Project preferences cannot override this choice.

AppKit changes the running Dock tile; Finder uses the shipped bundle icon.
The application does not edit signed bundles, Finder custom icons or installed
application files. Apple documents the runtime scope and clearing behavior in
[applicationIconImage](https://developer.apple.com/documentation/appkit/nsapplication/applicationiconimage),
and the bundle resource in
[CFBundleIconFile](https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundleiconfile).

For a nonactivating main-thread API/readback check:

```sh
cargo run -p pokarak --example app_icon_probe -- /tmp/forkarak-icon-readback
```

This verifies all seven choices against calibrated pixel renderings, image
lifetimes, actual preview decoding, `nil` restoration and off-main-thread
rejection. It does not substitute for interacting with the Settings dialog.
An optional second argument names the expected shipped ICNS, for testing the
same probe executable inside two isolated bundles with different defaults.
