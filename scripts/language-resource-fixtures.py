"""Authored tiny runtime resources for distribution tests. Never a shipping payload."""
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil


def preparation_module():
    spec = importlib.util.spec_from_file_location("language_preparation", Path(__file__).with_name("prepare-language-resources.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def create_fixture(root, executable):
    root = Path(root)
    java = root / "java/runtime/Contents/Home/bin/java"
    java.parent.mkdir(parents=True)
    shutil.copy2(executable, java)
    java.chmod(0o755)
    for name in ("legal/NOTICE.md", "legal/SOURCE-ACCESS.md", "legal/provenance.json", "sources/authored-source.txt", "recipes/kotlin.json"):
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('Authored distribution fixture; no third-party redistribution claim.\n' if not name.endswith('.json') else '{}\n')
    # Temurin includes read-only legal material. Packaging cleanup must finish
    # without an interactive rm prompt when replacing a prior local bundle.
    (root / "sources/authored-source.txt").chmod(0o444)
    tool = dict(package="java", version="authored-1", distribution="Bundled",
                artifacts=[dict(version="authored-1", source_url="https://download.eclipse.org/authored-fixture", checksum="sha256:" + hashlib.sha256(b"authored-fixture").hexdigest(), metadata_url="https://download.eclipse.org/authored-fixture.sha256")],
                servers=[dict(language="java", program="/usr/bin/env", args=["-i", "PATH=/usr/bin:/bin:/usr/sbin:/sbin", "FORKARAK_MANAGED=1", "{resources}/language-tools/java/runtime/Contents/Home/bin/java", "--version"])],
                formatters=[], notices=["{resources}/language-tools/legal/NOTICE.md", "{resources}/language-tools/legal/SOURCE-ACCESS.md"])
    (root / "manifest.json").write_text(json.dumps(dict(schemaVersion=1, packages=[tool]), indent=2) + "\n")
    prep = preparation_module()
    prep.inventory(root)
    prep.validate(root)
    return root
