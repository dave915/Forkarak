#!/usr/bin/env python3
"""Opt-in local package -> installer -> relocated real Java smoke verification.

No downloads, app activation, user app overwrite or release/push. Creates an
isolated HOME and authored curl transport around the supplied local signed app.
Native semantic/dirty-buffer tests belong to crates/navigation and crates/app.
Run this long archive test with hc run --bg, then hc wait.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tarfile
import tempfile

SCRIPTS = Path(__file__).resolve().parent


def run(app, keep=None):
    if keep and (not keep.is_absolute() or keep.exists() or keep.is_symlink() or keep.suffix != ".app"):
        raise ValueError("--keep-relocated-app requires a new absolute .app path")
    spec = importlib.util.spec_from_file_location("prepare", SCRIPTS / "prepare-language-resources.py")
    prep = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(prep)
    app = app.resolve(strict=True)
    version = plistlib.loads((app / "Contents/Info.plist").read_bytes())["CFBundleShortVersionString"]
    prep.validate(app / "Contents/Resources/language-tools")
    subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    with tempfile.TemporaryDirectory(prefix="forkarak-real-language-install-") as tmp:
        base = Path(tmp)
        archive = base / "forkarak-macos-arm64.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(app, arcname="Forkarak.app")
        (base / "SHA256SUMS").write_text(f"{prep.digest(archive)}  {archive.name}\n")
        mockbin = base / "mockbin"
        mockbin.mkdir()
        curl = mockbin / "curl"
        curl.write_text("#!" + sys.executable + "\n" + '''import os,pathlib,shutil,sys
args=sys.argv[1:]
url=args[args.index('--url')+1]
expected='https://github.com/dave915/Forkarak/releases/download/v'+os.environ['FORKARAK_FIXTURE_VERSION']+'/'
assert url.startswith(expected),url
name=url[len(expected):]
assert name in ('forkarak-macos-arm64.tar.gz','SHA256SUMS')
assert args[args.index('--proto')+1] == args[args.index('--proto-redir')+1] == '=https'
shutil.copyfile(pathlib.Path(os.environ['FORKARAK_FIXTURE_DIR'])/name,args[args.index('--output')+1])
sys.stdout.write('200 ')
''')
        curl.chmod(0o755)
        home = base / "empty home"
        home.mkdir()
        env = {"HOME": str(home), "PATH": str(mockbin) + ":/usr/bin:/bin:/usr/sbin:/sbin", "FORKARAK_FIXTURE_VERSION": version,
               "FORKARAK_FIXTURE_DIR": str(base), "DEVELOPER_DIR": str(base / "no-developer-tools")}
        result = subprocess.run(["/bin/bash", str(SCRIPTS / "install.sh"), "--version", "v" + version], env=env, capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        installed = home / "Applications/Forkarak.app"
        relocated = base / "Relocated 한글 Apps/Forkarak Local.app"
        relocated.parent.mkdir()
        installed.rename(relocated)
        resources = relocated / "Contents/Resources"
        root = resources / "language-tools"
        manifest = prep.validate(root)
        tool = manifest["packages"][0]
        assert tool["package"] == "java" and tool["distribution"] == "Bundled"
        assert not (root / "kotlin").exists()
        recipe = json.loads((root / "recipes/kotlin.json").read_text())
        assert not recipe["bundled"] and recipe["distribution"] == "FirstUseDownload"
        runtime = root / "java/runtime/Contents/Home/bin/java"
        result = subprocess.run(["/usr/bin/env", "-i", "PATH=/usr/bin:/bin:/usr/sbin:/sbin", str(runtime), "-version"], cwd=base,
                                env={"PATH": "/usr/bin:/bin", "DEVELOPER_DIR": env["DEVELOPER_DIR"]}, capture_output=True, text=True)
        assert result.returncode == 0, result.stdout + result.stderr
        assert 'Temurin-25.0.5+8' in result.stderr, result.stderr
        for artifact in json.loads((root / "legal/provenance.json").read_text())["artifacts"]:
            if artifact["destination"].startswith("sources/"):
                assert prep.digest(root / artifact["destination"]) == artifact["sha256"], artifact["name"]
        subprocess.run(["/bin/bash", str(resources / "verify-language-resources.sh"), str(root)], env={"PATH": "/usr/bin:/bin"}, check=True)
        subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(relocated)], check=True)
        prep.validate(root)  # Java startup did not mutate immutable Resources.
        print(f"PASS: default Java app {version} -> local installer -> relocated runtime with Finder PATH/no developer tools")
        print(f"PASS: {len(tool['artifacts'])} artifact provenance entries, exact source archive hashes, Kotlin first-use metadata, unchanged signed resources")
        print(result.stderr.strip())
        if keep:
            keep.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(relocated), keep)
            print(f"Preserved verified local test bundle: {keep}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True, help="Signed local Forkarak.app; never installed into user's HOME")
    parser.add_argument("--keep-relocated-app", type=Path, help="Preserve verified relocated fixture at a new absolute .app path")
    args = parser.parse_args()
    run(args.app, args.keep_relocated_app)
