#!/usr/bin/env python3
"""Exercise actual icon resources and stale-resource protections; no app launch."""
import hashlib
import importlib.util
import json
from pathlib import Path
import plistlib
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("icon_build", ROOT / "scripts/build-liquid-icons.py")
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class IconAssetsTests(unittest.TestCase):
    def test_native_catalog_has_all_six_real_lit_stacks(self):
        records = build.inspect_catalog(build.COMPILED / "Assets.car")
        for _, name, _ in build.sources.packages():
            stacks = [r for r in records if r.get("AssetType") == "IconImageStack" and r.get("Name") == name]
            self.assertTrue(stacks)
            self.assertTrue(all(r["LayerCount"] >= 2 for r in stacks))

    def test_all_presets_change_only_background(self):
        expected = None
        fills = set()
        for _, name, _ in build.sources.packages():
            package = build.sources.LIQUID / (name + ".icon")
            doc = json.loads((package / "icon.json").read_text())
            fills.add(json.dumps(doc.pop("fill"), sort_keys=True))
            current = (doc, (package / "Assets/spork.svg").read_bytes())
            if expected is None:
                expected = current
            self.assertEqual(current, expected)
        self.assertEqual(len(fills), 6)

    def test_actual_icns_roundtrips_all_ten_sizes(self):
        sizes = [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)]
        with tempfile.TemporaryDirectory(prefix="forkarak-icon-roundtrip-") as temporary:
            for preset, _, _ in build.sources.packages():
                output = Path(temporary) / (preset + ".iconset")
                subprocess.run(["/usr/bin/iconutil", "--convert", "iconset", "--output", str(output), str(build.APP / (preset + ".icns"))], check=True)
                self.assertEqual(len(list(output.glob("*.png"))), 10)
                for points, scale in sizes:
                    file = output / f"icon_{points}x{points}{'@2x' if scale == 2 else ''}.png"
                    png = file.read_bytes()
                    self.assertEqual(png[:8], b"\x89PNG\r\n\x1a\n")
                    self.assertEqual(struct.unpack(">II", png[16:24]), (points * scale,) * 2)

    def test_bundle_identity_and_legacy_default(self):
        plist = plistlib.loads((ROOT / "assets/macos/Info.plist").read_bytes())
        self.assertEqual(plist["CFBundleIconName"], "Forkarak")
        self.assertEqual(plist["CFBundleIconFile"], "Forkarak.icns")
        self.assertEqual(plist["CFBundleIdentifier"], "dev.pokarak.app")
        self.assertEqual(plist["LSMinimumSystemVersion"], "13.0")
        self.assertEqual((ROOT / "assets/icons/Forkarak.icns").read_bytes(), (build.APP / "charcoal.icns").read_bytes())

    def test_default_build_is_read_only_and_no_compile_permission_is_inferred(self):
        tracked = [p for p in (ROOT / "assets/icons").rglob("*") if p.is_file()]
        before = {p: hashlib.sha256(p.read_bytes()).hexdigest() for p in tracked}
        subprocess.run(["/bin/bash", str(ROOT / "scripts/build-icons.sh")], check=True, capture_output=True)
        result = subprocess.run([sys.executable, str(ROOT / "scripts/build-liquid-icons.py"), "--compile"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("explicit --write", result.stderr)
        self.assertEqual(before, {p: hashlib.sha256(p.read_bytes()).hexdigest() for p in tracked})

    def test_tampered_catalog_rejected_before_packaging(self):
        with tempfile.TemporaryDirectory(prefix="forkarak-tampered-icons-") as temporary:
            compiled = Path(temporary)
            for path in build.COMPILED.iterdir():
                shutil.copy2(path, compiled / path.name)
            with (compiled / "Assets.car").open("ab") as file:
                file.write(b"tampered")
            with self.assertRaisesRegex(ValueError, "checksum mismatch: Assets.car"):
                build.verify(compiled)

    def test_stale_sources_rejected_before_catalog_inspection(self):
        with tempfile.TemporaryDirectory(prefix="forkarak-stale-icons-") as temporary:
            compiled = Path(temporary)
            manifest = json.loads((build.COMPILED / "manifest.json").read_text())
            manifest["source-sha256"] = "0" * 64
            (compiled / "manifest.json").write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "catalog is stale"):
                build.verify(compiled)


if __name__ == "__main__":
    unittest.main()
