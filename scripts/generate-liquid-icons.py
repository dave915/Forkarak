#!/usr/bin/env python3
"""Generate official Icon Composer packages; render only with Apple's ictool.

The default is a read-only reproducibility check. --write explicitly replaces
generated source packages. --render-dir exports without changing shipping assets.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
LIQUID = ROOT / "assets/icons/app/liquid"
DESIGN = LIQUID / "source/design.json"


def color(value):
    return "srgb:" + ",".join(f"{int(value[i:i+2], 16) / 255:.5f}" for i in (1, 3, 5)) + ",1.00000"


def packages():
    design = json.loads(DESIGN.read_text())
    svg = (LIQUID / "source/spork.svg").read_bytes()
    for preset, entry in design["presets"].items():
        group = copy.deepcopy(design["material"])
        group["name"] = "Silver spork"
        group["layers"] = [{
            "name": "Four-tine spork", "image-name": "spork.svg",
            "glass": True, "hidden": False, "opacity": 1,
            "fill": design["foreground-fill"],
        }]
        document = {
            "fill": {"linear-gradient": list(map(color, entry["colors"]))},
            "groups": [group],
            "supported-platforms": {"squares": "shared"},
        }
        yield preset, entry["catalog-name"], {
            "icon.json": (json.dumps(document, indent=2) + "\n").encode(),
            "Assets/spork.svg": svg,
        }


def source_digest():
    digest = hashlib.sha256()
    for path in sorted(LIQUID.rglob("*")):
        if path.is_file():
            digest.update(path.relative_to(LIQUID).as_posix().encode() + b"\0")
            digest.update(path.read_bytes())
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="explicitly regenerate .icon packages")
    parser.add_argument("--render-dir", type=Path, help="export real Apple renders here (never into shipping paths)")
    parser.add_argument("--ictool", type=Path, help="Apple Icon Composer.app/Contents/Executables/ictool")
    parser.add_argument("--verify-renders", type=Path, help="verify exported PNGs and their source/tool provenance")
    args = parser.parse_args()
    expected_paths = set()
    for preset, name, files in packages():
        package = LIQUID / (name + ".icon")
        for relative, content in files.items():
            path = package / relative
            expected_paths.add(path)
            if args.write:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
            elif not path.is_file() or path.read_bytes() != content:
                raise ValueError(f"Stale source: {path}; review design then run --write explicitly")
        if args.render_dir:
            if not args.ictool or not args.ictool.is_file():
                raise ValueError("Rendering requires --ictool pointing to Apple's standalone Icon Composer")
            args.render_dir.mkdir(parents=True, exist_ok=True)
            output = args.render_dir / (preset + ".png")
            if output.exists():
                raise ValueError(f"Refusing to overwrite render: {output}")
            design = json.loads(DESIGN.read_text())
            subprocess.run([str(args.ictool.resolve()), str(package), "--export-image", "--output-file", str(output.resolve()),
                            "--platform", design["platform"], "--rendition", design["rendition"],
                            "--width", "1024", "--height", "1024", "--scale", "1",
                            "--light-angle", str(design["light-angle"])], check=True)
        print(f"Verified {preset}: {name}, shared spork/material")
    actual = {p for p in LIQUID.glob("*.icon/**/*") if p.is_file()}
    if actual != expected_paths:
        raise ValueError(f"Unexpected files in generated packages: {actual - expected_paths}")
    digest = source_digest()
    if args.render_dir:
        version = subprocess.check_output([str(args.ictool.resolve()), "--version"], text=True)
        manifest = {
            "source-sha256": digest,
            "renderer": "Apple Icon Composer ictool",
            "renderer-version": json.loads(version),
            "renderer-sha256": hashlib.sha256(args.ictool.read_bytes()).hexdigest(),
            "platform": "macOS", "rendition": "Default", "width": 1024, "height": 1024, "scale": 1,
            "light-angle": json.loads(DESIGN.read_text())["light-angle"],
            "images": {preset + ".png": hashlib.sha256((args.render_dir / (preset + ".png")).read_bytes()).hexdigest()
                       for preset, _, _ in packages()},
        }
        (args.render_dir / "render-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    if args.verify_renders:
        manifest = json.loads((args.verify_renders / "render-manifest.json").read_text())
        if manifest["source-sha256"] != digest:
            raise ValueError("Exports are stale relative to .icon source")
        for filename, expected in manifest["images"].items():
            if hashlib.sha256((args.verify_renders / filename).read_bytes()).hexdigest() != expected:
                raise ValueError(f"Export checksum mismatch: {filename}")
        if set(manifest["images"]) != {preset + ".png" for preset, _, _ in packages()}:
            raise ValueError("Exports must contain all six presets")
    print("Layered source SHA-256:", digest)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
