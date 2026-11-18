# Forkarak application icons

The macOS 26 default is a genuine layered Liquid Glass icon compiled from Apple's
Icon Composer format. Charcoal, deep blue, deep green, plum, ivory and light blue
share one four-tine spork, position, scale, silver fill, lighting, blur, specular,
translucency and shadow. Only the background colors differ.

## Editable source and provenance

`liquid/source/spork.svg` is an authored vector trace of the approved filled spork:
four short rounded tines, a spoon bowl and a short handle. The original user
reference and six approved designs remain untouched in `sources/`. Their source
thread is `01a1ea1f-8ba9-7d80-a252-2039dd203c02`; approvals are recorded in turns
`01a1ea35-ea02-7db2-b1c6-4a5c1c8e81bd` and
`01a1ea3a-1021-78c1-b794-5e92eee69f68`. Existing SVG logos and `vscode/` artwork are
separate assets. No third-party icon is substituted.

`liquid/source/design.json` holds the common material and six color pairs.
`liquid/Forkarak.icon` is the charcoal primary. The other packages are named
`Forkarak-deep-blue.icon`, `Forkarak-deep-green.icon`, `Forkarak-plum.icon`,
`Forkarak-ivory.icon` and `Forkarak-light-blue.icon`. Each is an ordinary Icon
Composer package (`icon.json` plus `Assets/spork.svg`). Root background fill and a
separate lit foreground group produce the native layered stack; artwork has no
baked shadow, blur or bevel. The package structure follows Apple's official
[Landmarks sample](https://developer.apple.com/documentation/swiftui/landmarks-building-an-app-with-liquid-glass).
Only the format was referenced, not its artwork.

```sh
# Read-only: confirm generated packages match common source.
python3 scripts/generate-liquid-icons.py
# Deliberate source regeneration after editing the common design.
python3 scripts/generate-liquid-icons.py --write
```

## Rendered fallbacks and documentation

`liquid-exports/` contains real 1024px macOS Default renders from Apple's standalone
Icon Composer `ictool` 1.0 (56), with a common 45-degree light angle. The render
manifest pins the renderer binary/version, source digest, parameters and PNG
hashes. Render without replacing existing output:

```sh
python3 scripts/generate-liquid-icons.py \
  --ictool '/path/to/Icon Composer.app/Contents/Executables/ictool' \
  --render-dir /tmp/forkarak-icon-exports
```

Six production PNGs, ICNS files and complete `.iconset` directories are flattened
exports of that same geometry/material. A shared 80/1024 inset supplies the
transparent legacy macOS frame. Each smaller representation is resampled directly
from its Apple render. These fixed images are **not native dynamic Liquid Glass**.
The charcoal export supplies common enclosure alpha coverage at each size to
avoid color-dependent edge rounding in CoreGraphics.
The originals in `sources/` are provenance, not the new generation input.

```sh
# Explicitly regenerate shipped fixed images from checked Apple exports.
swift scripts/generate-app-icons.swift --write-production \
  --qa-dir /tmp/forkarak-icon-qa
# QA-only rendering changes no shipping files.
swift scripts/generate-app-icons.swift --qa-dir /tmp/forkarak-icon-qa
```

The helper verifies all ten ICNS representations (16..1024px) by an actual
`iconutil` round trip. QA contact sheets use real images at 32/64/128px on light
and dark grounds. Optional overview sheets are written only to the QA directory.
`../Forkarak.icns` is the exact charcoal fallback; `../Forkarak-1024.png` and
`../app-icon-1024.png` are its PNG preview.

## Native catalog and packaging

`liquid-compiled/Assets.car` is the checked Apple `actool` output. The accompanying
manifest pins editable sources, all compiled resources and every shipped fixed
fallback. `assetutil-info.json`, compiler metadata, log and partial Info.plist
record the actual compilation and layered inventory. Verification requires six
1024-square `IconImageStack` identities with a background and lit/specular
foreground; an image-only catalog is rejected.

The checked catalog was compiled with Apple Xcode 26.6 (17F113), actool 26.6
(24765), macOS SDK 26.5. Local macOS 26.5.1 `assetutil` verified 18 native stacks:
six names × Aqua, Dark Aqua and tintable appearances, each with two layers.
All retain blur 0.45, shadow 0.38, translucency 0.12 and native specular lighting.
The compilation used an explicitly authorized temporary GitHub Actions branch
(run `44224612288`, source `a9d6dd2271dca750cb1fca2db4e8275afdb578a4`, output
`7dbbb504f6d04c42a155822ac6622d15a504d5c6`). All twelve compiler inputs were
byte-compared with these sources before importing the output. The parent owns
remote cleanup; no workflow or dependency on that temporary branch is shipped.

```sh
# Read-only verification, also used by packaging. No Xcode needed.
./scripts/build-icons.sh
# Rebuild with an existing Xcode 26+ installation; no xcode-select changes.
./scripts/build-icons.sh --compile --write \
  --developer-dir /Applications/Xcode.app/Contents/Developer
# Verify an assembled bundle without launching it.
python3 scripts/build-liquid-icons.py --bundle dist/Forkarak.app
```

`actool` is absent from standalone Command Line Tools. Recompiling the catalog
requires Xcode 26+, but packaging uses the checked catalog and existing ICNS;
normal user installation requires neither Xcode, Swift, Python nor compilation.
`package-macos.sh` verifies then copies the catalog as `Contents/Resources/Assets.car`
and preserves `Forkarak.icns`. The plist selects `CFBundleIconName=Forkarak` and
`CFBundleIconFile=Forkarak.icns`, keeps `dev.pokarak.app` and macOS 13.0 minimum.
An outdated source/resource pair fails packaging instead of silently falling back.
The old `build-icons.sh` SVG-overwrite behavior has been removed; generation and
catalog replacement always require explicit write flags.

Apple documents the format in
[Creating your app icon using Icon Composer](https://developer.apple.com/documentation/xcode/creating-your-app-icon-using-icon-composer)
and the bundle identity in
[CFBundleIconName](https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundleiconname).

## Runtime scope

The native default follows the installed bundle and system appearance. On macOS
26+, the runtime can load all six compiled names through public
`NSBundle.imageForResource`; an isolated bundle probe verified this catalog on
macOS 26.5.1. Explicit color overrides are native-rendered snapshots. Older
systems or unavailable catalog names use the matching fixed PNG exports. Loading
a named `NSImage` does not establish live dynamic Liquid Glass behavior for an
AppKit override. The app does not edit signed bundles or Finder custom icons.
`follow_default` clears the AppKit override with `nil`. Finder uses the packaged
default; explicit overrides affect the running Dock tile. The runtime worker
owns preview/loading behavior and native API probes.

```sh
python3 scripts/test-icon-assets.py
```
