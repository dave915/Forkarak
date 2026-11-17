#!/usr/bin/env python3
"""Offline installed-wrapper checks; no GUI, builds, network, or Git operations."""
import json
import os
from pathlib import Path
import plistlib
import shlex
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class LauncherTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="forkarak-cli-launcher-")
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name).resolve()
        self.cwd = self.base / "한글 caller ; [cwd]"
        self.cwd.mkdir()
        self.app = self.base / "apps ' 한글 $(touch unexpected)" / "Forkarak.app"
        self.binary = self.app / "Contents/MacOS/forkarak"
        self.binary.parent.mkdir(parents=True)
        resources = self.app / "Contents/Resources"
        resources.mkdir()
        self.bindir = self.base / "commands ' with spaces"
        self.bindir.mkdir()
        self.cli = self.bindir / "forkarak"
        self.log = self.base / "calls.jsonl"
        self.env = {
            **os.environ,
            "FORKARAK_FIXTURE_LOG": str(self.log),
            "FORKARAK_FIXTURE_EXIT": "0",
            "FORKARAK_FIXTURE_STDERR": "",
        }
        self.env.pop("FORKARAK_FIXTURE_STDOUT", None)
        recorder = '''import json,os,sys
call = {"kind": KIND, "argv": sys.argv[1:], "cwd": os.getcwd(), "pid": os.getpid()}
with open(os.environ["FORKARAK_FIXTURE_LOG"], "a", encoding="utf-8") as log:
    log.write(json.dumps(call, ensure_ascii=False) + "\\n")
response = json.dumps({"ok": True, "result": call}, ensure_ascii=False) + "\\n"
os.write(1, os.environ.get("FORKARAK_FIXTURE_STDOUT", response).encode("utf-8"))
os.write(2, os.environ["FORKARAK_FIXTURE_STDERR"].encode("utf-8"))
sys.exit(int(os.environ["FORKARAK_FIXTURE_EXIT"]))
'''
        for path, kind in ((self.binary, "app"), (self.base / "open", "open")):
            path.write_text(
                "#!" + sys.executable + "\nKIND = " + repr(kind) + "\n" + recorder,
                encoding="utf-8",
            )
            path.chmod(0o755)
        # The updater is run by Bash, just as the real installed resource is.
        (resources / "installer.py").write_text("KIND = 'installer'\n" + recorder, encoding="utf-8")
        (resources / "install.sh").write_text(
            "#!/bin/bash\nexec " + shlex.quote(sys.executable) + " "
            + shlex.quote(str(resources / "installer.py")) + ' "$@"\n',
            encoding="utf-8",
        )
        (self.app / "Contents/Info.plist").write_bytes(plistlib.dumps({
            "CFBundleShortVersionString": "0.1.0",
        }))
        wrapper = (ROOT / "scripts/forkarak-cli.sh").read_text(encoding="utf-8")
        # This wrapper must delegate all editor parsing and launching to native
        # code. The authored executable records argv and never launches a GUI.
        self.assertNotIn("/usr/bin/open", wrapper)
        # Match install.sh's managed launcher preamble; paths are shell-quoted.
        self.cli.write_text(
            "#!/bin/bash\n# Forkarak managed launcher\n"
            + "FORKARAK_APP_PATH=" + shlex.quote(str(self.app)) + "\n"
            + "FORKARAK_BIN_DIR=" + shlex.quote(str(self.bindir)) + "\n" + wrapper,
            encoding="utf-8",
        )
        self.cli.chmod(0o755)
        self.alias = self.bindir / "fkk"
        self.alias.write_bytes(self.cli.read_bytes())
        self.alias.chmod(0o755)

    def invoke(self, *args):
        process = subprocess.Popen(
            [str(self.cli), *args], cwd=self.cwd, env=self.env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.addCleanup(lambda: process.poll() is None and process.kill())
        stdout, stderr = process.communicate(timeout=5)
        return process.pid, subprocess.CompletedProcess(process.args, process.returncode, stdout, stderr)

    def calls(self):
        if not self.log.exists():
            return []
        return [json.loads(line) for line in self.log.read_text(encoding="utf-8").splitlines()]

    def test_ctl_exec_preserves_exact_argv_cwd_and_json(self):
        args = [
            "ctl", "search", "--repo", "../relative 한글 repo", "--query",
            "공백 ' double\" ; $(touch unexpected) `touch unexpected` * ? [a] & | < > $HOME \\",
            "", "line one\nline two", "--", "--help", "--version", "update", "ctl",
        ]
        pid, result = self.invoke(*args)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, b"")
        expected = {"kind": "app", "argv": args, "cwd": str(self.cwd), "pid": pid}
        self.assertEqual(self.calls(), [expected])
        self.assertEqual(json.loads(result.stdout), {"ok": True, "result": expected})
        self.assertEqual(len(result.stdout.splitlines()), 1)
        self.assertFalse((self.cwd / "unexpected").exists())

    def test_ctl_stdout_stderr_and_nonzero_exit_pass_through(self):
        for code in (1, 2, 37):
            with self.subTest(code=code):
                stdout = '{"ok":false,"error":{"code":"fixture","message":"한글 failure"}}\n'
                stderr = "fixture stderr 한글\n"
                self.env.update(FORKARAK_FIXTURE_STDOUT=stdout, FORKARAK_FIXTURE_STDERR=stderr,
                                FORKARAK_FIXTURE_EXIT=str(code))
                _, result = self.invoke("ctl", "status", "--repo", ".")
                self.assertEqual(result.returncode, code)
                self.assertEqual(result.stdout, stdout.encode("utf-8"))
                self.assertEqual(result.stderr, stderr.encode("utf-8"))
        self.assertEqual([call["kind"] for call in self.calls()], ["app"] * 3)

    def test_ctl_help_and_agent_guides_are_forwarded(self):
        for args in (("ctl",), ("ctl", "--help"),
                     ("ctl", "instructions", "--agent", "claude"),
                     ("ctl", "instructions", "--agent", "codex")):
            with self.subTest(args=args):
                _, result = self.invoke(*args)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["result"]["argv"], list(args))
        self.assertTrue(all(call["kind"] == "app" for call in self.calls()))

    def test_ctl_missing_or_unusable_executable_is_actionable(self):
        self.binary.unlink()
        for state in ("missing", "not executable", "directory"):
            with self.subTest(state=state):
                if state == "not executable":
                    self.binary.write_text("fixture", encoding="utf-8")
                    self.binary.chmod(0o644)
                elif state == "directory":
                    self.binary.unlink()
                    self.binary.mkdir()
                _, result = self.invoke("ctl", "status", "--repo", ".")
                self.assertEqual(result.returncode, 1)
                self.assertEqual(result.stdout, b"")
                self.assertIn(str(self.binary).encode("utf-8"), result.stderr)
                self.assertIn(b"run install.sh again", result.stderr)
                self.assertEqual(self.calls(), [])

    def test_editor_launch_preserves_exact_args_cwd_and_alias(self):
        absolute = self.base / "absolute 한글 file.rs"
        for command in (self.cli, self.alias):
            for paths in ((), ("relative file.rs", "한글 ; $(touch unexpected)", str(absolute)),
                          ("--", "ctl", "update", "--version", "-literal"),
                          ("--reuse-window", "-g", "name:12:3", "second file", "--background"),
                          ("--new-window", "--window-size", "1100x700", "--session-dir", "state dir", ".")):
                with self.subTest(command=command.name, paths=paths):
                    process = subprocess.run([str(command), *paths], cwd=self.cwd, env=self.env,
                                             capture_output=True, timeout=5)
                    self.assertEqual(process.returncode, 0, process.stderr)
                    call = self.calls()[-1]
                    self.assertEqual(call["kind"], "app")
                    self.assertEqual(call["argv"], ["--cli-open", str(self.app), *paths])
                    self.assertEqual(call["cwd"], str(self.cwd))
        self.assertFalse((self.cwd / "unexpected").exists())

    def test_alias_ctl_update_help_and_version_are_identical(self):
        for args in (("ctl", "help"), ("ctl", "status", "--repo", "."),
                     ("install-cli", "--bin-dir", "custom 한글 commands"),
                     ("update", "--version", "v1.2.3"), ("--help",), ("--version",)):
            with self.subTest(args=args):
                results = [subprocess.run([str(command), *args], cwd=self.cwd, env=self.env,
                                          capture_output=True, timeout=5)
                           for command in (self.cli, self.alias)]
                self.assertEqual(results[0].returncode, results[1].returncode)
                if args[0] in ("--help", "--version"):
                    self.assertEqual(results[0].stdout, results[1].stdout)
                else:
                    calls = self.calls()[-2:]
                    self.assertEqual(calls[0]["kind"], calls[1]["kind"])
                    self.assertEqual(calls[0]["argv"], calls[1]["argv"])

    def test_update_preserves_installed_directories_and_arguments(self):
        for extra, code in (((), 0), (("--version", "v1.2.3"), 23), (("--help",), 0)):
            with self.subTest(extra=extra):
                self.env["FORKARAK_FIXTURE_EXIT"] = str(code)
                _, result = self.invoke("update", *extra)
                self.assertEqual(result.returncode, code, result.stderr)
                call = self.calls()[-1]
                self.assertEqual(call["kind"], "installer")
                self.assertEqual(call["argv"], ["--app-dir", str(self.app.parent),
                                               "--bin-dir", str(self.bindir), *extra])
                self.assertEqual(call["cwd"], str(self.cwd))

    def test_version_uses_installed_plist_and_rejects_extra_arguments(self):
        _, result = self.invoke("--version")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, b"0.1.0\n")
        _, result = self.invoke("--version", "extra")
        self.assertEqual(result.returncode, 1)
        self.assertIn(b"--version takes no arguments", result.stderr)
        self.assertEqual(self.calls(), [])

    def test_help_explains_ctl_and_both_agent_guides(self):
        for option in ("--help", "-h", "help"):
            with self.subTest(option=option):
                _, result = self.invoke(option)
                self.assertEqual(result.returncode, 0, result.stderr)
                help_text = result.stdout.decode("utf-8")
                for text in ("forkarak [OPTIONS] [--] [FILE|DIRECTORY ...]", "fkk [OPTIONS]", "forkarak ctl --help",
                             "forkarak ctl status --repo .", "instructions --agent claude",
                             "instructions --agent codex", "JSON", "result.instructions",
                             "forkarak update [--version vX.Y.Z]", "forkarak --version",
                             "--goto FILE:LINE[:COLUMN]", "--background", "--no-launch", "install-cli [--bin-dir DIRECTORY]",
                             "Loose files/new file creation", "--wait and --diff are not supported"):
                    self.assertIn(text, help_text)
        self.assertEqual(self.calls(), [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
