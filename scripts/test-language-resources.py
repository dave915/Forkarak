#!/usr/bin/env python3
"""Authored archive/resource fixtures; no downloads or user repositories."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("fixtures", SCRIPTS / "language-resource-fixtures.py")
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)
prep = fixtures.preparation_module()


class LanguageResourceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="forkarak-language-fixtures-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)

    def archive(self, entries):
        path = self.base / "authored.tar.gz"
        with tarfile.open(path, "w:gz") as tar:
            for name, kind, data in entries:
                member = tarfile.TarInfo(name)
                member.mode = 0o755
                if kind == "file":
                    member.size = len(data)
                    tar.addfile(member, io.BytesIO(data))
                else:
                    member.type = {"sym": tarfile.SYMTYPE, "hard": tarfile.LNKTYPE, "dir": tarfile.DIRTYPE, "fifo": tarfile.FIFOTYPE}[kind]
                    member.linkname = data
                    tar.addfile(member)
        return path

    def test_internal_file_links_are_materialized_with_executable_permissions(self):
        a = self.archive([("jdk/bin/java", "file", b"authored"), ("jdk/bin/javac", "sym", "java"), ("jdk/java", "hard", "jdk/bin/java"), ("jdk/bin/chain", "sym", "../java")])
        out = self.base / "extracted"
        prep.safe_extract(a, out)
        for name in ("jdk/bin/java", "jdk/bin/javac", "jdk/java", "jdk/bin/chain"):
            p = out / name
            self.assertEqual(p.read_bytes(), b"authored")
            self.assertFalse(p.is_symlink())
            self.assertEqual(p.stat().st_mode & 0o777, 0o755)

    def test_traversal_special_duplicate_and_unsafe_links_are_rejected_before_files(self):
        cases = [
            [("../outside", "file", b"bad")], [("/absolute", "file", b"bad")],
            [("jdk/../outside", "file", b"bad")], [("jdk\\outside", "file", b"bad")],
            [("jdk/new\nline", "file", b"bad")], [("jdk/pipe", "fifo", "")],
            [("jdk/a", "file", b"a"), ("jdk/a", "file", b"b")],
            [("jdk/a", "sym", "/etc/passwd")], [("jdk/a", "hard", "../outside")],
            [("jdk/a", "sym", "../../outside")], [("jdk/a", "sym", "b"), ("jdk/b", "sym", "a")],
            [("jdk/a", "sym", "missing")], [("jdk/a", "dir", ""), ("jdk/link", "sym", "a")],
            [("jdk/a", "sym", "b"), ("jdk/a/child", "file", b"bad"), ("jdk/b", "file", b"ok")],
        ]
        for i, case in enumerate(cases):
            with self.subTest(case=case):
                out = self.base / f"extracted-{i}"
                with self.assertRaises(ValueError):
                    prep.safe_extract(self.archive(case), out)
                self.assertEqual(list(out.rglob("*")), [])

    def test_artifact_cache_verifies_hashes_and_never_trusts_corrupt_or_linked_cache(self):
        cache = self.base / "cache"
        cache.mkdir()
        a = dict(name="fixture.tar.gz", sha256=hashlib.sha256(b"authored").hexdigest(), url="https://download.eclipse.org/authored.tar.gz")
        (cache / a["name"]).write_bytes(b"authored")
        with patch.object(prep.subprocess, "run", side_effect=AssertionError("Cached bytes must not trigger network")):
            self.assertEqual(prep.download(a, cache, True), cache / a["name"])
        (cache / a["name"]).write_bytes(b"corrupt")
        with self.assertRaises(ValueError):
            prep.download(a, cache, True)
        (cache / a["name"]).unlink()
        (cache / a["name"]).symlink_to(self.base / "outside")
        with self.assertRaises(ValueError):
            prep.download(a, cache)
        for url in ("http://download.eclipse.org/a", "https://evil.invalid/a", "https://download.eclipse.org@evil.invalid/a"):
            with self.assertRaises(ValueError):
                prep.download({**a, "url": url}, cache)

    def test_corrupt_download_is_not_staged_and_previous_cache_is_preserved(self):
        a = dict(name="fixture.tar.gz", sha256=hashlib.sha256(b"expected").hexdigest(), url="https://download.eclipse.org/authored.tar.gz")
        cache = self.base / "cache"
        cache.mkdir()
        old = cache / a["name"]
        old.write_bytes(b"previous cache")
        def corrupt(argv, **kwargs):
            self.assertIn("=https", argv)
            Path(argv[argv.index("--output") + 1]).write_bytes(b"corrupt download")
        with patch.object(prep.subprocess, "run", side_effect=corrupt):
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                prep.download(a, cache)
        self.assertEqual(old.read_bytes(), b"previous cache")
        self.assertEqual(list(cache.iterdir()), [old])

    def test_valid_cached_artifact_is_reused_without_network_or_subprocess(self):
        cache = self.base / "cache"
        cache.mkdir()
        path = cache / "fixture"
        path.write_bytes(b"verified")
        artifact = dict(name="fixture", sha256=hashlib.sha256(b"verified").hexdigest(), url="https://download.eclipse.org/authored-fixture")
        with patch.object(prep.subprocess, "run") as run:
            self.assertEqual(prep.download(artifact, cache), path)
            run.assert_not_called()

    def test_failed_preparation_retains_previous_resources(self):
        root = self.resource_fixture()
        before = (root / "SHA256SUMS").read_bytes()
        # A download or extraction failure must occur before publication.
        with patch.object(prep, "download", side_effect=ValueError("checksum mismatch")):
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                prep.prepare(root.parent, self.base / "cache")
        self.assertEqual((root / "SHA256SUMS").read_bytes(), before)
        prep.validate(root)

    def test_extraction_refuses_a_symlink_destination(self):
        outside = self.base / "outside"
        outside.mkdir()
        link = self.base / "extracted"
        link.symlink_to(outside)
        with self.assertRaises(ValueError):
            prep.safe_extract(self.archive([("file", "file", b"bad")]), link)
        self.assertEqual(list(outside.iterdir()), [])

    def resource_fixture(self):
        exe = self.base / "java-fixture"
        exe.write_bytes(b"authored executable")
        exe.chmod(0o755)
        return fixtures.create_fixture(self.base / "Resources/language-tools", exe)

    def test_resource_manifest_is_relocatable_and_inventories_sources_notices(self):
        root = self.resource_fixture()
        relocated = self.base / "Relocated 한글 App.app/Contents/Resources/language-tools"
        relocated.parent.mkdir(parents=True)
        root.rename(relocated)
        manifest = prep.validate(relocated)
        self.assertEqual(manifest["packages"][0]["distribution"], "Bundled")
        self.assertNotIn(str(self.base), (relocated / "manifest.json").read_text())
        self.assertIn("sources/authored-source.txt", (relocated / "SHA256SUMS").read_text())
        result = subprocess.run(["/bin/bash", str(SCRIPTS / "verify-language-resources.sh"), str(relocated)], env={"PATH": "/usr/bin:/bin"}, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_corrupt_missing_unlisted_unsafe_nonrelocatable_resources_are_rejected(self):
        original = self.resource_fixture()
        def corrupt(root): (root / "legal/NOTICE.md").write_text("changed")
        def missing(root): (root / "sources/authored-source.txt").unlink()
        def extra(root): (root / "extra").write_text("unlisted")
        def link(root): (root / "linked").symlink_to("legal/NOTICE.md")
        def checksum(root): (root / "SHA256SUMS").write_text("0" * 64 + "  ../outside\n")
        def manifest(root, program="{resources}/language-tools/../escape", package="java"):
            j = json.loads((root / "manifest.json").read_text());j["packages"][0]["package"] = package;j["packages"][0]["servers"][0]["program"] = program
            (root / "manifest.json").write_text(json.dumps(j));prep.inventory(root)
        def absolute_env(root):
            j = json.loads((root / "manifest.json").read_text());j["packages"][0]["servers"][0]["args"].insert(1, "JAVA_HOME=/opt/homebrew/opt/openjdk")
            (root / "manifest.json").write_text(json.dumps(j));prep.inventory(root)
        for i, mutate in enumerate([corrupt, missing, extra, link, checksum, manifest, absolute_env, lambda r: manifest(r, "/opt/homebrew/bin/java"), lambda r: manifest(r, "/usr/bin/env", "kotlin")]):
            with self.subTest(mutation=i):
                root = self.base / f"mutated-{i}"
                shutil.copytree(original, root)
                mutate(root)
                with self.assertRaises((ValueError, OSError)):
                    prep.validate(root)
                result = subprocess.run(["/bin/bash", str(SCRIPTS / "verify-language-resources.sh"), str(root)], capture_output=True)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
