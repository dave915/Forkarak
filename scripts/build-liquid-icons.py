#!/usr/bin/env python3
"""Verify or explicitly rebuild the shipped layered icon catalog.

Verification and packaging need Python + system assetutil, never Xcode. Rebuilding
requires an explicitly selected Xcode 26+ developer directory. No global selection
changes. --write is required before importing/replacing the checked catalog.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
APP = ROOT / "assets/icons/app"
COMPILED = APP / "liquid-compiled"
spec = importlib.util.spec_from_file_location("liquid_sources", ROOT / "scripts/generate-liquid-icons.py")
sources = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sources)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fallback_paths():
    paths = [ROOT / "assets/icons/Forkarak.icns", ROOT / "assets/icons/Forkarak-1024.png", ROOT / "assets/icons/app-icon-1024.png"]
    for preset, _, _ in sources.packages():
        paths += [APP / (preset + ".icns"), APP / (preset + ".png")]
        paths += sorted((APP / (preset + ".iconset")).glob("*.png"))
    return paths


def inspect_catalog(path):
    records = json.loads(subprocess.check_output(["/usr/bin/assetutil", "--info", str(path)]))
    names = [name for _, name, _ in sources.packages()]
    for name in names:
        stacks = [r for r in records if r.get("AssetType") == "IconImageStack" and r.get("Name") == name]
        if len(stacks) != 3 or any(r.get("LayerCount") != 2 or r.get("CanvasWidth") != 1024 or r.get("CanvasHeight") != 1024 for r in stacks):
            raise ValueError(f"Missing layered 1024-square IconImageStack: {name}")
        groups = [r for r in records if r.get("AssetType") == "IconGroup" and r.get("Name", "").startswith(name + "/")]
        if not groups or not any(layer.get("LayerHasLightingEffects") for g in groups for layer in g.get("Layers", [])):
            raise ValueError(f"Missing lit foreground group: {name}")
        if not any(layer.get("LayerHasSpecular") for stack in stacks for layer in stack.get("Layers", [])):
            raise ValueError(f"Missing native specular material: {name}")
        appearances = {stack.get("Appearance") for stack in stacks}
        if appearances != {"NSAppearanceNameAqua", "NSAppearanceNameDarkAqua", "ISAppearanceTintable"}:
            raise ValueError(f"Missing native appearance: {name}: {appearances}")
        material = json.loads(sources.DESIGN.read_text())["material"]
        for stack in stacks:
            for layer in stack.get("Layers", []):
                if layer.get("AssetType") != "IconGroup":
                    continue
                if not layer.get("LayerHasSpecular") or layer.get("LayerPosition") != "0,0" or layer.get("LayerOpacity") != 1:
                    raise ValueError(f"Inconsistent foreground composition: {name}")
                for key, expected in {"LayerBlurStrength": material["blur-material"],
                                      "LayerShadowOpacity": material["shadow"]["opacity"],
                                      "LayerTranslucency": material["translucency"]["value"]}.items():
                    if abs(layer.get(key, -1) - expected) > 0.00001:
                        raise ValueError(f"Inconsistent compiled material: {name} {key}")
    return records


def validate_sources():
    subprocess.run([sys.executable, str(ROOT / "scripts/generate-liquid-icons.py"), "--verify-renders", str(APP / "liquid-exports")], check=True)


def seal(incoming, compiler_metadata):
    validate_sources()
    records = inspect_catalog(incoming / "Assets.car")
    partial = plistlib.loads((incoming / "partial-info.plist").read_bytes())
    if partial.get("CFBundleIconName") != "Forkarak":
        raise ValueError("Compiler partial plist does not select Forkarak as primary")
    compiler = json.loads(compiler_metadata.read_text())
    if not compiler.get("xcode-version") or not compiler.get("actool-command"):
        raise ValueError("Compiler metadata requires xcode-version and actool-command")
    version = re.search(r"\bXcode\s+(\d+)", compiler["xcode-version"])
    if not version or int(version.group(1)) < 26:
        raise ValueError("Compiler provenance must identify Xcode 26 or newer")
    COMPILED.mkdir(parents=True, exist_ok=True)
    shutil.copy2(incoming / "Assets.car", COMPILED / "Assets.car")
    shutil.copy2(incoming / "partial-info.plist", COMPILED / "partial-info.plist")
    (COMPILED / "assetutil-info.json").write_text(json.dumps(records, indent=2) + "\n")
    (COMPILED / "compiler-metadata.json").write_text(json.dumps(compiler, indent=2) + "\n")
    if (incoming / "actool.log").is_file():
        shutil.copy2(incoming / "actool.log", COMPILED / "actool.log.txt")
    if (incoming / "toolchain.txt").is_file():
        shutil.copy2(incoming / "toolchain.txt", COMPILED / "toolchain.txt")
    manifest = {
        "format": 1, "primary-icon": "Forkarak", "minimum-system-version": "13.0",
        "source-sha256": sources.source_digest(),
        "catalog-names": [name for _, name, _ in sources.packages()],
        "files": {p.name: sha(p) for p in sorted(COMPILED.iterdir()) if p.name != "manifest.json"},
        "fallbacks": {p.relative_to(ROOT).as_posix(): sha(p) for p in fallback_paths()},
    }
    (COMPILED / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


def verify(compiled=COMPILED, bundle=None):
    validate_sources()
    manifest = json.loads((compiled / "manifest.json").read_text())
    if manifest.get("format") != 1 or manifest.get("primary-icon") != "Forkarak" or manifest.get("minimum-system-version") != "13.0":
        raise ValueError("Compiled icon manifest contract mismatch")
    if manifest["source-sha256"] != sources.source_digest():
        raise ValueError("Layered catalog is stale; explicitly rebuild with Xcode 26+")
    if manifest["catalog-names"] != [name for _, name, _ in sources.packages()]:
        raise ValueError("Catalog name contract mismatch")
    required = {"Assets.car", "partial-info.plist", "assetutil-info.json", "compiler-metadata.json"}
    if not required.issubset(manifest["files"]):
        raise ValueError("Compiled resource inventory is incomplete")
    if {p.name for p in compiled.iterdir()} != set(manifest["files"]) | {"manifest.json"}:
        raise ValueError("Unexpected compiled resource inventory")
    for filename, expected in manifest["files"].items():
        if sha(compiled / filename) != expected:
            raise ValueError(f"Compiled resource checksum mismatch: {filename}")
    expected_fallbacks = {p.relative_to(ROOT).as_posix() for p in fallback_paths()}
    if set(manifest["fallbacks"]) != expected_fallbacks:
        raise ValueError("Fallback inventory mismatch")
    for filename, expected in manifest["fallbacks"].items():
        if sha(ROOT / filename) != expected:
            raise ValueError(f"Fallback checksum mismatch: {filename}")
    inspect_catalog(compiled / "Assets.car")
    plist = plistlib.loads((ROOT / "assets/macos/Info.plist").read_bytes())
    if bundle:
        plist = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
        for name in ["Assets.car", "Forkarak.icns"]:
            source = compiled / name if name == "Assets.car" else ROOT / "assets/icons" / name
            if sha(bundle / "Contents/Resources" / name) != sha(source):
                raise ValueError(f"Packaged resource mismatch: {name}")
        inspect_catalog(bundle / "Contents/Resources/Assets.car")
    for key, expected in {"CFBundleIconName": "Forkarak", "CFBundleIconFile": "Forkarak.icns",
                          "CFBundleIdentifier": "dev.pokarak.app", "LSMinimumSystemVersion": "13.0"}.items():
        if plist.get(key) != expected:
            raise ValueError(f"Incorrect bundle {key}: {plist.get(key)!r}")
    print("Verified six native layered stacks, source/resource hashes, bundle identity and macOS 13 ICNS fallback")


def compile_catalog(developer_dir):
    env = dict(os.environ)
    if developer_dir:
        env["DEVELOPER_DIR"] = str(developer_dir)
    try:
        version = subprocess.check_output(["/usr/bin/xcrun", "xcodebuild", "-version"], env=env, text=True)
        major = int(version.splitlines()[0].split()[1].split(".")[0])
        if major < 26:
            raise ValueError("Xcode 26 or newer is required")
        subprocess.run(["/usr/bin/xcrun", "--find", "actool"], env=env, check=True, capture_output=True)
    except subprocess.CalledProcessError as error:
        raise ValueError("Native catalog compilation requires Xcode 26+; CLT alone has no actool. Set --developer-dir to an existing supported Xcode, without changing xcode-select.") from error
    with tempfile.TemporaryDirectory(prefix="forkarak-layered-icons-") as temp:
        output = Path(temp)
        command = ["/usr/bin/xcrun", "actool", "--compile", str(output), "--output-format", "human-readable-text",
                   "--notices", "--warnings", "--platform", "macosx", "--minimum-deployment-target", "13.0",
                   "--target-device", "mac", "--app-icon", "Forkarak", "--include-all-app-icons"]
        names = sorted(name for _, name, _ in sources.packages())
        for name in names:
            if name != "Forkarak":
                command += ["--alternate-app-icon", name]
        command += ["--output-partial-info-plist", str(output / "partial-info.plist")]
        command += [str(sources.LIQUID / (name + ".icon")) for name in names]
        result = subprocess.run(command, env=env, capture_output=True, text=True)
        (output / "actool.log").write_text(result.stdout + result.stderr)
        if result.returncode:
            sys.stderr.write(result.stdout + result.stderr)
            raise ValueError("actool compilation failed; checked catalog unchanged")
        metadata = {"xcode-version": version.strip(), "actool-command": command,
                    "sdk-version": subprocess.check_output(["/usr/bin/xcrun", "--sdk", "macosx", "--show-sdk-version"], env=env, text=True).strip()}
        (output / "compiler-metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        seal(output, output / "compiler-metadata.json")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compile", action="store_true")
    parser.add_argument("--developer-dir", type=Path)
    parser.add_argument("--import-compiled", type=Path, help="import parent-supplied Apple actool output")
    parser.add_argument("--compiler-metadata", type=Path)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--bundle", type=Path, help="also verify packaged bundle without launching it")
    args = parser.parse_args()
    if args.compile and args.import_compiled:
        parser.error("Choose --compile or --import-compiled")
    if args.compile or args.import_compiled:
        if not args.write:
            parser.error("Replacing checked resources requires explicit --write")
        if args.compile:
            compile_catalog(args.developer_dir)
        else:
            seal(args.import_compiled, args.compiler_metadata or args.import_compiled / "compiler-metadata.json")
    verify(bundle=args.bundle)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, FileNotFoundError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
