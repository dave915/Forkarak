#!/usr/bin/env python3
"""Exercise packaging and installation against signed disposable app fixtures."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import select
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
import zipfile

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("language_fixtures", ROOT / "scripts/language-resource-fixtures.py")
language_fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(language_fixtures)


class DistributionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.sandbox = tempfile.TemporaryDirectory(prefix="forkarak-distribution-")
        cls.base = Path(cls.sandbox.name)
        cls.source = cls.base / "한글 source with spaces"
        cls.source.mkdir()
        shutil.copytree(ROOT / "scripts", cls.source / "scripts")
        shutil.copytree(ROOT / "assets", cls.source / "assets")
        shutil.copy2(ROOT / "LICENSE", cls.source / "LICENSE")
        # The authored fixture version stays stable across application releases.
        (cls.source / "Cargo.toml").write_text('[workspace]\n[workspace.package]\nversion = "0.1.0"\n')
        cls.args_file = cls.base / "launched-arguments.txt"
        cls.launched_exe = cls.base / "launched-executable.txt"
        cfile = cls.base / "stub.c"
        cfile.write_text(
            '#include <stdio.h>\n#include <string.h>\n#include <unistd.h>\nint main(int argc,char **argv) {'
            f'FILE *exe=fopen({json.dumps(str(cls.launched_exe))},"w");'
            'if(!exe)return 1;fputs(argv[0],exe);fclose(exe);'
            f'FILE *f=fopen({json.dumps(str(cls.args_file))},"w");'
            'if(!f)return 1;for(int i=1;i<argc;i++)fprintf(f,"%s\\n",argv[i]);'
            'fclose(f);if(argc>1 && !strcmp(argv[1],"--hold"))sleep(60);return 0;}\n'
        )
        cls.binary = cls.base / "stub"
        subprocess.run(["/usr/bin/cc", "-arch", "arm64", "-mmacosx-version-min=13.0", str(cfile), "-o", str(cls.binary)], check=True, capture_output=True)
        runtime_c = cls.base / "runtime.c"
        runtime_c.write_text('#include <stdio.h>\nint main(void){puts("authored-java-runtime");return 0;}\n')
        runtime = cls.base / "authored-java"
        subprocess.run(["/usr/bin/cc", "-arch", "arm64", "-mmacosx-version-min=13.0", str(runtime_c), "-o", str(runtime)], check=True, capture_output=True)
        cls.language_resources = language_fixtures.create_fixture(cls.base / "authored-language-tools", runtime)
        result = subprocess.run(
            ["/bin/bash", str(cls.source / "scripts/release-macos.sh"), "--version", "v0.1.0", "--binary", str(cls.binary)],
            capture_output=True, text=True, env={**os.environ, "SIGNING_IDENTITY": "", "NOTARY_KEYCHAIN_PROFILE": "", "FORKARAK_LANGUAGE_RESOURCES": str(cls.language_resources)},
        )
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        cls.good_release = cls.source / "dist/release"
        cls.mockbin = cls.base / "mockbin"
        cls.mockbin.mkdir()
        # Only curl is replaced. Every validation/packaging tool is the actual
        # system binary. This maps production URLs onto authored fixture assets.
        curl = cls.mockbin / "curl"
        # Use the actual interpreter, not the /usr/bin/python3 CLT shim, so the
        # curl fixture still works when the installer simulates a clean Mac.
        curl.write_text("#!" + sys.executable + "\n" + '''import os,pathlib,shutil,sys
args=sys.argv[1:]
assert args[0] == '-q'
url=args[args.index('--url')+1]
assert args[args.index('--proto')+1] == args[args.index('--proto-redir')+1] == '=https'
assert args[args.index('--write-out')+1] == '%{http_code} %{redirect_url}'
with open(os.environ['FORKARAK_CURL_LOG'],'a') as f: f.write(url+'\\n')
releases='https://github.com/dave915/Forkarak/releases'
failure=os.environ.get('FORKARAK_FAIL_DOWNLOAD')
if failure in ('1',url) or url.startswith('https://api.github.com/'):
    sys.stdout.write('403 ')
    sys.exit(22)
if url == releases+'/latest':
    assert '--head' in args and '--no-location' in args
    tag=os.environ.get('FORKARAK_FIXTURE_TAG','v0.1.0')
    dest=os.environ.get('FORKARAK_FIXTURE_REDIRECT',releases+'/tag/'+tag)
    sys.stdout.write(os.environ.get('FORKARAK_FIXTURE_HTTP','302')+' '+dest)
else:
    assert '--location' in args and url.startswith(releases+'/download/')
    out=pathlib.Path(args[args.index('--output')+1])
    shutil.copyfile(pathlib.Path(os.environ['FORKARAK_FIXTURE_RELEASE'])/url.rsplit('/',1)[1],out)
    sys.stdout.write('200 ')
''')
        curl.chmod(0o755)

    @classmethod
    def tearDownClass(cls):
        cls.sandbox.cleanup()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=self.base, prefix="case-")
        self.addCleanup(self.temporary.cleanup)
        self.case = Path(self.temporary.name)
        self.home = self.case / "한글 home"
        self.home.mkdir()
        self.appdir = self.home / "Applications"
        self.bindir = self.home / ".local/bin"
        self.app = self.appdir / "Forkarak.app"
        self.cli = self.bindir / "forkarak"
        self.log = self.case / "curl.log"
        self.release = self.case / "release"
        shutil.copytree(self.good_release, self.release)
        self.env = {**os.environ, "HOME": str(self.home), "PATH": str(self.mockbin) + ":" + os.environ["PATH"], "FORKARAK_FIXTURE_RELEASE": str(self.release), "FORKARAK_CURL_LOG": str(self.log), "FORKARAK_LANGUAGE_RESOURCES": str(self.language_resources)}

    def install(self, *args, expected=0):
        result = subprocess.run(["/bin/bash", str(self.source / "scripts/install.sh"), *args], capture_output=True, text=True, env=self.env)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        if "Another install" not in result.stderr:
            self.assertFalse((self.appdir / ".forkarak-install.lock").exists(), "Installer left its lock behind")
        return result

    def test_check_only_is_read_only_and_emits_only_an_official_stable_tag(self):
        self.env["HOME"] = str(self.case / "nonexistent home")
        # The mock curl uses Python. Keep its bytecode cache out of the test HOME.
        self.env["PYTHONDONTWRITEBYTECODE"] = "1"
        self.assertEqual(self.install("--check-only").stdout, "v0.1.0\n")
        self.assertEqual(self.log.read_text().splitlines(), ["https://github.com/dave915/Forkarak/releases/latest"])
        self.env["FORKARAK_FIXTURE_REDIRECT"] = "https://example.org/tag/v0.1.0"
        self.assertEqual(self.install("--check-only", expected=1).stdout, "")
        self.assertFalse(Path(self.env["HOME"]).exists())
        self.log.unlink()
        for args in (("--check-only", "--version", "v2.0.0"), ("--app-dir", str(self.case), "--check-only"),
                     ("--wait-pid", "1"), ("--relaunch-session-dir", "/tmp")):
            self.install(*args, expected=1)
        self.assertFalse(self.log.exists())

    def zip_install(self):
        self.appdir = self.case / "custom apps"
        self.appdir.mkdir()
        self.app = self.appdir / """개발자 ' " $(touch nope).app"""
        subprocess.run(["/usr/bin/ditto", "-x", "-k", str(self.release / "Forkarak-macos-arm64.zip"),
                        str(self.appdir)], check=True, capture_output=True)
        (self.appdir / "Forkarak.app").rename(self.app)
        self.assertTrue((self.app / "Contents/Resources/update-handoff.sh").is_file())

    def hold_app(self):
        process = subprocess.Popen([str(self.app / "Contents/MacOS/forkarak"), "--hold"])

        def stop():
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=5)

        self.addCleanup(stop)
        return process

    def app_update(self, pid, *extra):
        script = self.app / "Contents/Resources/install.sh"
        process = subprocess.Popen(["/bin/bash", str(script), "--update-running-app", str(self.app),
                                    "--wait-pid", str(pid), "--version", "v0.1.0", *extra],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   bufsize=0, start_new_session=True, env=self.env)

        def stop():
            if process.stdin and not process.stdin.closed:
                process.stdin.close()
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=5)
            process.stdout.close()
            process.stderr.close()

        self.addCleanup(stop)
        return process

    def wait_marker(self, process, marker):
        data = b""
        end = time.monotonic() + 20
        while time.monotonic() < end:
            if not select.select([process.stdout], [], [], max(0, end - time.monotonic()))[0]:
                break
            chunk = os.read(process.stdout.fileno(), 4096)
            if not chunk:
                self.fail(f"Missing {marker}: {data!r} {process.stderr.read()!r}")
            data += chunk
            if marker.encode() in data.split(b"\n")[:-1]:
                return
        self.fail(f"Timed out waiting for {marker}: {data!r}")

    def test_zip_update_reopens_exact_bundle_with_session_after_pipes_close(self):
        self.zip_install()
        legacy = self.appdir / "포카락.app"
        shutil.copytree(self.app, legacy)
        legacy_inode = legacy.stat().st_ino
        self.bindir.mkdir(parents=True)
        self.cli.write_text("unrelated command")
        self.env["DEVELOPER_DIR"] = str(self.case / "no developer tools")
        session = self.case / """세션 -- ' $ HOME"""
        session.mkdir()
        original = self.app.stat().st_ino
        old = self.hold_app()
        update = self.app_update(old.pid, "--relaunch-session-dir", str(session))
        self.wait_marker(update, "POKARAK_UPDATE_READY")
        self.assertEqual(self.app.stat().st_ino, original)
        self.assertTrue((self.appdir / ".forkarak-install.lock").is_dir())
        self.assertIn("Another install", self.install("--app-dir", str(self.appdir), expected=1).stderr)
        holders = subprocess.run(["/usr/sbin/lsof", "-a", "-p", str(update.pid),
                                  str(self.app / "Contents/Resources/install.sh")], capture_output=True)
        self.assertEqual(holders.returncode, 1, "Update process is still using the old installer")
        update.stdin.write(b"restart\n")
        self.wait_marker(update, "POKARAK_UPDATE_RESTARTING")
        update.stdin.close()
        update.stdout.close()
        update.stderr.close()
        self.assertEqual(self.app.stat().st_ino, original)
        old.terminate()
        old.wait(timeout=5)
        self.assertEqual(update.wait(timeout=20), 0, (self.home / "Library/Logs/Forkarak/update.log").read_text())
        for _ in range(100):
            if self.args_file.read_text().splitlines() == ["--session-dir", str(session)]:
                break
            time.sleep(0.05)
        self.assertEqual(self.args_file.read_text().splitlines(), ["--session-dir", str(session)])
        self.assertTrue(self.app.joinpath("Contents/MacOS/forkarak").samefile(self.launched_exe.read_text()))
        self.assertNotEqual(self.app.stat().st_ino, original)
        self.assertEqual(legacy.stat().st_ino, legacy_inode)
        self.assertEqual(self.cli.read_text(), "unrelated command")
        self.assertFalse(list(self.appdir.glob(".forkarak-*")))
        self.assertTrue(all("/download/v0.1.0/" in url for url in self.log.read_text().splitlines()))
        subprocess.run(["/usr/bin/codesign", "--verify", "--strict", str(self.app)], check=True)

    def test_running_update_cancels_before_quit_or_modification(self):
        self.zip_install()
        original = self.app.stat().st_ino
        old = self.hold_app()
        for answer, status in [(b"", 0), (b"anything else\n", 1)]:
            with self.subTest(answer=answer):
                update = self.app_update(old.pid)
                self.wait_marker(update, "POKARAK_UPDATE_READY")
                stdout, _ = update.communicate(answer, timeout=5)
                self.assertEqual(update.returncode, status)
                self.assertNotIn(b"RESTARTING", stdout)
                self.assertEqual(self.app.stat().st_ino, original)
                self.assertIsNone(old.poll())
                self.assertFalse(list(self.appdir.glob(".forkarak-*")))
        self.assertFalse(self.bindir.exists())
        self.assertFalse((self.home / "Applications").exists())

    def test_running_update_rejects_bad_pid_options_and_busy_bundle(self):
        self.zip_install()
        old = self.hold_app()
        second = self.hold_app()
        for pid, extra in [(old.pid, []), (os.getpid(), []), (old.pid, ["--relaunch-session-dir", "relative"]),
                           (old.pid, ["--bin-dir", str(self.case / "bin")]), ("-1", [])]:
            with self.subTest(pid=pid, extra=extra):
                updater = self.app_update(pid, *extra)
                stdout, _ = updater.communicate(timeout=10)
                self.assertNotEqual(updater.returncode, 0)
                self.assertNotIn(b"UPDATE_READY", stdout)
        self.assertFalse(self.log.exists())
        self.assertIsNone(old.poll())
        self.assertIsNone(second.poll())
        self.assertFalse(list(self.appdir.glob(".forkarak-*")))

    def test_running_update_rechecks_other_instances_on_both_sides_of_ack(self):
        self.zip_install()
        original = self.app.stat().st_ino
        for after_ack in (False, True):
            with self.subTest(after_ack=after_ack):
                old = self.hold_app()
                updater = self.app_update(old.pid)
                self.wait_marker(updater, "POKARAK_UPDATE_READY")
                if not after_ack:
                    second = self.hold_app()
                updater.stdin.write(b"restart\n")
                if after_ack:
                    self.wait_marker(updater, "POKARAK_UPDATE_RESTARTING")
                    second = self.hold_app()
                    old.terminate()
                    old.wait(timeout=5)
                stdout, stderr = updater.communicate(timeout=10)
                self.assertEqual(updater.returncode, 1)
                if after_ack:
                    self.assertIn("Another instance opened", (self.home / "Library/Logs/Forkarak/update.log").read_text())
                else:
                    self.assertNotIn(b"RESTARTING", stdout)
                    self.assertIn(b"Quit other", stderr)
                    self.assertIsNone(old.poll())
                    old.terminate()
                    old.wait(timeout=5)
                self.assertIsNone(second.poll())
                second.terminate()
                second.wait(timeout=5)
                self.assertEqual(self.app.stat().st_ino, original)
                self.assertFalse(list(self.appdir.glob(".forkarak-*")))

    def test_failed_relaunch_restores_and_reopens_original_with_session(self):
        self.zip_install()
        # Simulate LaunchServices rejecting the new bundle precisely once. The
        # rollback and reopening of the original use the real system tools.
        helper = self.app / "Contents/Resources/update-handoff.sh"
        helper.write_text(helper.read_text().replace(
            "reopen_app() {", 'reopen_app() {\n    [[ "$(bundle_identity "$APP_PATH")" == "$ORIGINAL_ID" ]] || return 1'))
        subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", str(self.app)], check=True, capture_output=True)
        old_identity = (self.app.stat().st_ino, (self.app / "Contents/MacOS/forkarak").stat().st_ino)
        old = self.hold_app()
        session = self.case / "recovery session"
        session.mkdir()
        updater = self.app_update(old.pid, "--relaunch-session-dir", str(session))
        self.wait_marker(updater, "POKARAK_UPDATE_READY")
        updater.stdin.write(b"restart\n")
        self.wait_marker(updater, "POKARAK_UPDATE_RESTARTING")
        old.terminate()
        old.wait(timeout=5)
        self.assertEqual(updater.wait(timeout=10), 1)
        log = (self.home / "Library/Logs/Forkarak/update.log").read_text()
        self.assertIn("LaunchServices could not reopen", log)
        self.assertIn("Restored the previous Forkarak", log)
        self.assertEqual((self.app.stat().st_ino, (self.app / "Contents/MacOS/forkarak").stat().st_ino), old_identity)
        for _ in range(100):
            if self.args_file.read_text().splitlines() == ["--session-dir", str(session)]:
                break
            time.sleep(0.05)
        self.assertEqual(self.args_file.read_text().splitlines(), ["--session-dir", str(session)])
        self.assertTrue(self.app.joinpath("Contents/MacOS/forkarak").samefile(self.launched_exe.read_text()))
        self.assertFalse(list(self.appdir.glob(".forkarak-*")))

    def test_running_update_rejects_incompatible_macos_before_ready(self):
        self.zip_install()
        old = self.hold_app()
        app = self.case / "future/Forkarak.app"
        shutil.copytree(self.app, app)
        plist = app / "Contents/Info.plist"
        data = plistlib.loads(plist.read_bytes())
        data["LSMinimumSystemVersion"] = "99.0"
        plist.write_bytes(plistlib.dumps(data))
        self.repack(app)
        update = self.app_update(old.pid)
        stdout, stderr = update.communicate(timeout=10)
        self.assertEqual(update.returncode, 1)
        self.assertNotIn(b"UPDATE_READY", stdout)
        self.assertIn(b"requires macOS 99.0", stderr)
        self.assertIsNone(old.poll())
        self.assertFalse(list(self.appdir.glob(".forkarak-*")))

    def test_timeout_preserves_the_instance_that_did_not_quit(self):
        self.zip_install()
        old = self.hold_app()
        original = self.app.stat().st_ino
        update = self.app_update(old.pid)
        self.wait_marker(update, "POKARAK_UPDATE_READY")
        update.stdin.write(b"restart\n")
        self.wait_marker(update, "POKARAK_UPDATE_RESTARTING")
        self.assertEqual(update.wait(timeout=40), 1)
        self.assertIn("Timed out waiting", (self.home / "Library/Logs/Forkarak/update.log").read_text())
        self.assertEqual(self.app.stat().st_ino, original)
        self.assertIsNone(old.poll())
        self.assertFalse(list(self.appdir.glob(".forkarak-*")))

    def test_locked_running_app_is_preserved_before_ready(self):
        self.zip_install()
        subprocess.run(["/usr/bin/chflags", "uchg", str(self.app)], check=True)
        self.addCleanup(subprocess.run, ["/usr/bin/chflags", "nouchg", str(self.app)], check=True)
        old = self.hold_app()
        update = self.app_update(old.pid)
        stdout, stderr = update.communicate(timeout=10)
        self.assertEqual(update.returncode, 1)
        self.assertNotIn(b"UPDATE_READY", stdout)
        self.assertIn(b"cannot be replaced", stderr)
        self.assertIsNone(old.poll())
        self.assertFalse(list(self.appdir.glob(".forkarak-*")))

    def test_running_update_fails_before_ready_on_bad_checksum(self):
        self.zip_install()
        original = self.app.stat().st_ino
        old = self.hold_app()
        (self.release / "SHA256SUMS").write_text("bad")
        updater = self.app_update(old.pid)
        stdout, stderr = updater.communicate(timeout=10)
        self.assertNotIn(b"UPDATE_READY", stdout)
        self.assertNotEqual(updater.returncode, 0)
        self.assertIn(b"checksum", stderr)
        self.assertIsNone(old.poll())
        self.assertEqual(self.app.stat().st_ino, original)
        self.assertFalse(list(self.appdir.glob(".forkarak-*")))
        self.assertFalse(self.bindir.exists())

    def test_install_and_pinned_downloads(self):
        self.install()
        self.assertTrue(self.app.is_dir())
        version = subprocess.check_output([str(self.cli), "--version"], text=True, env=self.env).strip()
        self.assertEqual(version, "0.1.0")
        urls = self.log.read_text().splitlines()
        self.assertEqual(len(urls), 3)
        self.assertEqual(urls[0], "https://github.com/dave915/Forkarak/releases/latest")
        self.assertTrue(all("/releases/download/v0.1.0/" in u for u in urls[1:]))
        self.assertFalse((self.appdir / ".forkarak-install.lock").exists())

    def test_pinned_version_bypasses_latest_lookup(self):
        self.env["FORKARAK_FIXTURE_TAG"] = "v9.9.9"
        self.install("--version", "v0.1.0")
        self.assertEqual(self.log.read_text().splitlines(), [
            "https://github.com/dave915/Forkarak/releases/download/v0.1.0/forkarak-macos-arm64.tar.gz",
            "https://github.com/dave915/Forkarak/releases/download/v0.1.0/SHA256SUMS",
        ])

    def test_latest_redirect_requires_published_stable_tag_in_this_repository(self):
        latest = "https://github.com/dave915/Forkarak/releases/latest"
        for status, location in [
            ("200", ""),
            ("302", "https://other.example/dave915/Forkarak/releases/tag/v0.1.0"),
            ("302", "https://github.com/other/forkarak/releases/tag/v0.1.0"),
            ("302", "https://github.com/dave915/Forkarak/releases/tag/v0.1.0-beta.1"),
            ("302", "https://github.com/dave915/Forkarak/releases/tag/v0.01.0"),
            ("302", "https://github.com/dave915/Forkarak/releases/tag/v0.1.0/other"),
        ]:
            with self.subTest(location=location):
                self.env.update(FORKARAK_FIXTURE_HTTP=status, FORKARAK_FIXTURE_REDIRECT=location)
                self.log.unlink(missing_ok=True)
                self.install(expected=1)
                self.assertEqual(self.log.read_text().splitlines(), [latest])
                self.assertFalse(self.app.exists())
                self.assertFalse(self.cli.exists())

    def test_finder_download_has_english_bundle_and_display_names(self):
        with zipfile.ZipFile(self.release / "Forkarak-macos-arm64.zip") as archive:
            plist = plistlib.loads(archive.read("Forkarak.app/Contents/Info.plist"))
            self.assertFalse(any("포카락.app/" in entry for entry in archive.namelist()))
            self.assertIn("Forkarak.app/Contents/MacOS/forkarak", archive.namelist())
            self.assertIn("Forkarak.app/Contents/Resources/Forkarak.icns", archive.namelist())
            self.assertIn("Forkarak.app/Contents/Resources/forkarak-cli.sh", archive.namelist())
        self.assertEqual(plist["CFBundleExecutable"], "forkarak")
        self.assertEqual(plist["CFBundleIconFile"], "Forkarak.icns")
        self.assertEqual(plist["CFBundleDisplayName"], "Forkarak")
        self.assertEqual(plist["CFBundleName"], "Forkarak")
        self.assertEqual(plist["CFBundleIdentifier"], "dev.pokarak.app")
        with tarfile.open(self.release / "forkarak-macos-arm64.tar.gz") as archive:
            self.assertEqual({name.split("/")[0] for name in archive.getnames()}, {"Forkarak.app"})
        expected_files = {"Forkarak-macos-arm64.zip", "forkarak-macos-arm64.tar.gz", "install.sh", "SHA256SUMS"}
        self.assertEqual({file.name for file in self.release.iterdir()}, expected_files)
        with (self.release / "SHA256SUMS").open() as checksums:
            for line in checksums:
                digest, filename = line.split()
                self.assertEqual(digest, hashlib.sha256((self.release / filename).read_bytes()).hexdigest())

    def test_fresh_install_leaves_previous_product_and_command_untouched(self):
        self.appdir.mkdir()
        self.bindir.mkdir(parents=True)
        old_cli = self.bindir / "pokarak"
        old_cli.write_text("# Pokarak managed launcher\n")
        old_apps = [self.appdir / "Pokarak.app", self.appdir / "포카락.app"]
        for bundle in old_apps:
            contents = bundle / "Contents"
            contents.mkdir(parents=True)
            (contents / "Info.plist").write_bytes(plistlib.dumps({
                "CFBundleIdentifier": "dev.pokarak.app", "CFBundleExecutable": "pokarak",
            }))
        old_inodes = [path.stat().st_ino for path in [old_cli, *old_apps]]
        self.install()
        self.assertTrue(self.app.is_dir())
        self.assertTrue(self.cli.is_file())
        self.assertEqual(old_cli.read_text(), "# Pokarak managed launcher\n")
        self.assertEqual([p.stat().st_ino for p in [old_cli, *old_apps]], old_inodes)

    def test_refuses_another_executable_with_the_shared_bundle_identifier(self):
        contents = self.app / "Contents"
        contents.mkdir(parents=True)
        plist = contents / "Info.plist"
        data = plistlib.dumps({"CFBundleIdentifier": "dev.pokarak.app", "CFBundleExecutable": "pokarak"})
        plist.write_bytes(data)
        self.assertIn("not a Forkarak bundle", self.install(expected=1).stderr)
        self.assertEqual(plist.read_bytes(), data)
        self.assertFalse(self.log.exists())
        self.assertFalse(self.cli.exists())

    def test_update_preserves_custom_directories(self):
        self.appdir = self.case / "custom apps"
        self.bindir = self.case / "custom commands"
        self.app = self.appdir / "Forkarak.app"
        self.cli = self.bindir / "forkarak"
        self.install("--app-dir", str(self.appdir), "--bin-dir", str(self.bindir))
        marker = self.app / "old-installation-marker"
        marker.write_text("old")
        new_app = self.case / "new release/Forkarak.app"
        shutil.copytree(self.source / "dist/Forkarak.app", new_app)
        plist = new_app / "Contents/Info.plist"
        data = plistlib.loads(plist.read_bytes())
        data.update(CFBundleShortVersionString="0.1.1", CFBundleVersion="0.1.1")
        plist.write_bytes(plistlib.dumps(data))
        self.repack(new_app)
        self.env["FORKARAK_FIXTURE_TAG"] = "v0.1.1"
        result = subprocess.run([str(self.cli), "update"], env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(marker.exists())
        self.assertTrue(self.cli.exists())
        self.assertEqual(subprocess.check_output([str(self.cli), "--version"], env=self.env, text=True).strip(), "0.1.1")
        self.assertFalse((self.home / "Applications/Forkarak.app").exists())

    def test_checksum_failure_preserves_app_and_launcher(self):
        self.install()
        old_plist = (self.app / "Contents/Info.plist").read_bytes()
        old_cli = self.cli.read_bytes()
        (self.release / "forkarak-macos-arm64.tar.gz").write_bytes(b"corrupt download")
        result = self.install(expected=1)
        self.assertIn("checksum verification failed", result.stderr)
        self.assertEqual((self.app / "Contents/Info.plist").read_bytes(), old_plist)
        self.assertEqual(self.cli.read_bytes(), old_cli)

    def test_update_reports_failing_public_url_and_preserves_install(self):
        self.install()
        previous_inodes = (self.app.stat().st_ino, self.cli.stat().st_ino)
        previous_app = (self.app / "Contents/MacOS/forkarak").read_bytes()
        previous_cli = self.cli.read_bytes()
        releases = "https://github.com/dave915/Forkarak/releases"
        for path, step in [
            ("latest", "Latest release lookup"),
            ("download/v0.1.0/forkarak-macos-arm64.tar.gz", "App archive download"),
            ("download/v0.1.0/SHA256SUMS", "Checksum download"),
        ]:
            with self.subTest(path=path):
                url = releases + "/" + path
                self.env["FORKARAK_FAIL_DOWNLOAD"] = url
                result = subprocess.run([str(self.cli), "update"], env=self.env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 22)
                self.assertIn(f"{step} failed at {url} (HTTP 403", result.stderr)
                self.assertEqual((self.app / "Contents/MacOS/forkarak").read_bytes(), previous_app)
                self.assertEqual(self.cli.read_bytes(), previous_cli)
                self.assertEqual((self.app.stat().st_ino, self.cli.stat().st_ino), previous_inodes)
                self.assertFalse((self.appdir / ".forkarak-install.lock").exists())

    def repack(self, app, sign=True):
        if sign:
            subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", str(app)], check=True, capture_output=True)
        archive = self.release / "forkarak-macos-arm64.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(app, arcname="Forkarak.app")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.release / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n")

    def test_installed_then_relocated_runtime_uses_manifest_with_finder_path(self):
        self.env["PATH"] = str(self.mockbin) + ":/usr/bin:/bin:/usr/sbin:/sbin"
        self.env["DEVELOPER_DIR"] = str(self.case / "no-developer-tools")
        self.install()
        relocated = self.case / "Relocated 한글 Apps/Local Forkarak.app"
        relocated.parent.mkdir()
        self.app.rename(relocated)
        resources = relocated / "Contents/Resources"
        root = resources / "language-tools"
        manifest = language_fixtures.preparation_module().validate(root)
        command = manifest["packages"][0]["servers"][0]
        args = [arg.replace("{resources}", str(resources)) for arg in command["args"]]
        result = subprocess.run([command["program"], *args], env={"PATH": "/usr/bin:/bin", "DEVELOPER_DIR": self.env["DEVELOPER_DIR"]}, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "authored-java-runtime")
        for file in ("legal/NOTICE.md", "legal/SOURCE-ACCESS.md", "sources/authored-source.txt"):
            self.assertIn(file, (root / "SHA256SUMS").read_text())
        subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(relocated)], check=True, capture_output=True)

    def test_invalid_language_resources_preserve_existing_app_and_runtime(self):
        self.install()
        previous_app = (self.app / "Contents/MacOS/forkarak").read_bytes()
        previous_cli = self.cli.read_bytes()
        previous_runtime = (self.app / "Contents/Resources/language-tools/java/runtime/Contents/Home/bin/java").read_bytes()
        for kind in ("corrupt", "missing", "unsafe-manifest", "missing-tree"):
            with self.subTest(kind=kind):
                app = self.case / kind / "Forkarak.app"
                shutil.copytree(self.source / "dist/Forkarak.app", app)
                root = app / "Contents/Resources/language-tools"
                if kind == "corrupt":
                    (root / "legal/NOTICE.md").write_text("corrupt notice")
                elif kind == "missing":
                    (root / "sources/authored-source.txt").unlink()
                elif kind == "missing-tree":
                    shutil.rmtree(root)
                else:
                    path = root / "manifest.json"
                    j = json.loads(path.read_text())
                    j["packages"][0]["servers"][0]["args"][3] = "{resources}/language-tools/../../MacOS/forkarak"
                    path.write_text(json.dumps(j))
                    language_fixtures.preparation_module().inventory(root)
                self.repack(app)
                result = self.install(expected=1)
                self.assertIn("anguage resource", result.stderr)
                self.assertEqual((self.app / "Contents/MacOS/forkarak").read_bytes(), previous_app)
                self.assertEqual(self.cli.read_bytes(), previous_cli)
                self.assertEqual((self.app / "Contents/Resources/language-tools/java/runtime/Contents/Home/bin/java").read_bytes(), previous_runtime)

    def test_packaging_rejects_corrupt_prepared_resources_without_replacing_bundle(self):
        bad = self.case / "bad-prepared"
        shutil.copytree(self.language_resources, bad)
        (bad / "legal/NOTICE.md").unlink()
        app = self.source / "dist/Forkarak.app"
        before = (app / "Contents/_CodeSignature/CodeResources").read_bytes()
        result = subprocess.run(["/bin/bash", str(self.source / "scripts/package-macos.sh"), "--binary", str(self.binary)], env={**self.env, "FORKARAK_LANGUAGE_RESOURCES": str(bad)}, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((app / "Contents/_CodeSignature/CodeResources").read_bytes(), before)

    def test_invalid_bundle_and_version_rejected(self):
        for key, value in (("CFBundleIdentifier", "invalid.app"), ("CFBundleShortVersionString", "9.9.9")):
            with self.subTest(key=key):
                app = self.case / key / "Forkarak.app"
                shutil.copytree(self.source / "dist/Forkarak.app", app)
                plist = app / "Contents/Info.plist"
                data = plistlib.loads(plist.read_bytes())
                data[key] = value
                plist.write_bytes(plistlib.dumps(data))
                self.repack(app)
                self.install(expected=1)
                self.assertFalse(self.app.exists())

    def test_unrelated_destinations_and_symlinks_refused(self):
        self.appdir.mkdir()
        self.app.symlink_to(self.case / "unrelated")
        self.assertIn("symlink", self.install(expected=1).stderr)
        self.app.unlink()
        self.app.mkdir()
        self.assertIn("not a Forkarak", self.install(expected=1).stderr)
        self.app.rmdir()
        self.bindir.mkdir(parents=True, exist_ok=True)
        self.cli.write_text("#!/bin/bash\necho unrelated\n")
        self.assertIn("unrelated command", self.install(expected=1).stderr)
        self.assertIn("unrelated", self.cli.read_text())

    def test_existing_lock_is_preserved(self):
        self.appdir.mkdir()
        lock = self.appdir / ".forkarak-install.lock"
        lock.mkdir()
        self.assertIn("Another install", self.install(expected=1).stderr)
        self.assertTrue(lock.exists())

    def test_unsafe_archive_rejected_before_extraction(self):
        archive = self.release / "forkarak-macos-arm64.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            info = tarfile.TarInfo("Forkarak.app/Contents/escape")
            info.type = tarfile.SYMTYPE
            info.linkname = "/tmp"
            tar.addfile(info)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.release / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n")
        self.assertIn("links or special", self.install(expected=1).stderr)
        self.assertFalse(self.app.exists())

    def test_cli_launch_preserves_unicode_and_relative_paths(self):
        self.install()
        self.args_file.unlink(missing_ok=True)
        paths = ["한글 project", "second with spaces"]
        paths.append("-literal")
        result = subprocess.run([str(self.cli), "--", *paths], cwd=self.case, env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        for _ in range(40):
            if self.args_file.exists():
                break
            time.sleep(0.05)
        self.assertTrue(self.args_file.exists(), "LaunchServices did not execute the authored fixture")
        self.assertEqual(self.args_file.read_text().splitlines(), ["--", *[str(self.case.resolve() / p) for p in paths]])

    def test_running_installed_app_is_refused(self):
        self.install()
        process = subprocess.Popen([str(self.app / "Contents/MacOS/forkarak"), "--hold"])
        try:
            time.sleep(0.1)
            self.assertIsNone(process.poll())
            result = self.install(expected=1)
            self.assertIn("Quit Forkarak", result.stderr)
            self.assertTrue(self.app.is_dir())
        finally:
            process.terminate()
            process.wait(timeout=5)

    def test_option_errors_do_not_download(self):
        for args in (("--bad",), ("--version",), ("--version", "../bad")):
            self.install(*args, expected=1)
        self.assertFalse(self.log.exists())
        result = subprocess.run(["/bin/bash", str(self.source / "scripts/release-macos.sh"), "--version", "v9.9.9", "--binary", str(self.binary)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("workspace version", result.stderr)

    def test_installer_works_without_developer_tools(self):
        self.env["DEVELOPER_DIR"] = str(self.case / "developer-tools-not-installed")
        shim = subprocess.run(["/usr/bin/lipo", "-archs", str(self.binary)], env=self.env, capture_output=True)
        self.assertNotEqual(shim.returncode, 0, "Fixture must disable the lipo developer-tools shim")
        self.install()
        self.assertEqual(subprocess.check_output([str(self.cli), "--version"], env=self.env, text=True).strip(), "0.1.0")

    def test_wrong_architecture_and_non_macho_are_rejected(self):
        intel = self.case / "intel-stub"
        subprocess.run(["/usr/bin/cc", "-arch", "x86_64", "-mmacosx-version-min=13.0", str(self.base / "stub.c"), "-o", str(intel)], check=True, capture_output=True)
        self.assertEqual(subprocess.check_output(["/usr/bin/file", "-b", str(intel)], env={**os.environ, "LC_ALL": "C"}, text=True).strip(), "Mach-O 64-bit executable x86_64")
        invalid = self.case / "invalid-executable"
        invalid.write_text("#!/bin/bash\necho not-a-Mach-O\n")
        invalid.chmod(0o755)
        for name, binary in (("intel", intel), ("invalid", invalid)):
            with self.subTest(executable=name):
                app = self.case / name / "Forkarak.app"
                shutil.copytree(self.source / "dist/Forkarak.app", app)
                shutil.copy2(binary, app / "Contents/MacOS/forkarak")
                self.repack(app, sign=False)
                self.assertIn("not an arm64 Mach-O executable", self.install(expected=1).stderr)
                self.assertFalse(self.app.exists())

    def test_release_builds_without_binary_on_system_bash(self):
        cargo_bin = self.case / "cargo-bin"
        cargo_bin.mkdir()
        target = self.case / "authored-target"
        executable = target / "aarch64-apple-darwin/release/pokarak"
        executable.parent.mkdir(parents=True)
        shutil.copy2(self.binary, executable)
        log = self.case / "cargo-arguments.jsonl"
        cargo = cargo_bin / "cargo"
        cargo.write_text("#!/usr/bin/env python3\n" + '''import json,os,sys
with open(os.environ['FORKARAK_CARGO_LOG'],'a') as f:
    f.write(json.dumps(sys.argv[1:])+'\\n')
if sys.argv[1] == 'metadata':
    print(json.dumps({'target_directory':os.environ['FORKARAK_CARGO_TARGET']}))
elif sys.argv[1] != 'build':
    sys.exit(2)
''')
        cargo.chmod(0o755)
        env = {**self.env, "PATH": str(cargo_bin) + ":" + self.env["PATH"], "FORKARAK_CARGO_LOG": str(log), "FORKARAK_CARGO_TARGET": str(target), "SIGNING_IDENTITY": "", "NOTARY_KEYCHAIN_PROFILE": ""}
        result = subprocess.run(["/bin/bash", str(self.source / "scripts/release-macos.sh"), "--version", "v0.1.0"], capture_output=True, text=True, env=env)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        commands = [json.loads(line) for line in log.read_text().splitlines()]
        self.assertEqual([cmd[0] for cmd in commands], ["build", "metadata"])
        self.assertIn("--locked", commands[0])
        self.assertIn("--release", commands[0])
        self.assertIn("aarch64-apple-darwin", commands[0])
        self.assertTrue((self.source / "dist/release/forkarak-macos-arm64.tar.gz").is_file())


if __name__ == "__main__":
    unittest.main(verbosity=2)
