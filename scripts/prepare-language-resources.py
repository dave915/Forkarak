#!/usr/bin/env python3
"""Build-side only: prepare pinned, relocatable Java support and its legal material.

The installed application uses native ManagedTools; it never runs this Python file.
No tool or project is executed during preparation. Use hc run --bg for downloads.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import zipfile

LOCK = Path(__file__).with_name("language-resources.lock.json")
TOKEN = "{resources}/language-tools/"


def digest(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def relative(value):
    if not isinstance(value, str) or not value or any(c in value for c in "\\\r\n\0"):
        raise ValueError("Unsafe resource path")
    p = PurePosixPath(value)
    if p.is_absolute() or any(c in ("", ".", "..") for c in value.split("/")):
        raise ValueError(f"Unsafe resource path: {value}")
    return p


def download(artifact, cache, offline=False):
    # URL and digest are committed together after inspecting official metadata.
    # Never resolve mutable latest versions during an ordinary package build.
    url = artifact["url"]
    from urllib.parse import urlparse
    host = urlparse(url)
    if host.scheme != "https" or host.username or host.password or host.hostname not in {
        "github.com", "codeload.github.com", "raw.githubusercontent.com",
        "download.eclipse.org", "www.eclipse.org", "repo.maven.apache.org",
        "www.jetbrains.com", "www.apache.org",
    }:
        raise ValueError(f"Not an approved official source: {url}")
    expected = artifact["sha256"]
    if not re.fullmatch("[0-9a-f]{64}", expected):
        raise ValueError("Invalid pinned SHA-256")
    name = str(relative(artifact["name"]))
    if "/" in name:
        raise ValueError("Artifact cache name must be a filename")
    target = cache / name
    if target.is_symlink():
        raise ValueError("Artifact cache contains a symlink")
    if target.exists() and digest(target) == expected:
        return target
    if offline:
        raise ValueError(f"Missing or corrupt cached artifact: {name}")
    cache.mkdir(parents=True, exist_ok=True)
    fd, scratch = tempfile.mkstemp(prefix=".download-", dir=cache)
    os.close(fd)
    try:
        subprocess.run(["/usr/bin/curl", "-q", "--fail", "--location", "--silent", "--show-error",
                        "--proto", "=https", "--proto-redir", "=https", "--tlsv1.2",
                        "--connect-timeout", "15", "--max-time", "900", "--retry", "2",
                        "--output", scratch, "--url", url], check=True)
        if digest(scratch) != expected:
            raise ValueError(f"Artifact checksum mismatch: {name}")
        os.replace(scratch, target)
        return target
    finally:
        Path(scratch).unlink(missing_ok=True)


def safe_extract(archive, output):
    """Extract regular files; materialize only verified internal file links.

    No tar extraction API touches the filesystem. Inspect the entire graph first,
    reject duplicate/special entries, links as parents, escaping and cyclic links.
    A link can only become a copy of a regular file in this same archive.
    """
    output = Path(output)
    if output.is_symlink():
        raise ValueError("Extraction destination is a symlink")
    if output.exists() and any(output.iterdir()):
        raise ValueError("Extraction destination must be empty")
    output.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, "r:*") as t:
        entries = {}
        for m in t:
            name = m.name.removeprefix("./").rstrip("/")
            if not name and m.isdir():
                continue
            relative(name)
            if name in entries or not (m.isfile() or m.isdir() or m.issym() or m.islnk()):
                raise ValueError(f"Duplicate or special archive member: {name}")
            entries[name] = m
        if sum(m.size for m in entries.values()) > 8 * 1024**3:
            raise ValueError("Archive is too large")
        for name in entries:
            for parent in PurePosixPath(name).parents:
                m = entries.get(str(parent))
                if m and not m.isdir():
                    raise ValueError("Archive member has non-directory parent")

        def target_for(name, visiting):
            if name in visiting or name not in entries:
                raise ValueError("Missing or cyclic archive link")
            m = entries[name]
            if m.isfile():
                return m
            if not (m.issym() or m.islnk()):
                raise ValueError("Only internal links to regular files may be normalized")
            link = m.linkname
            if not link or link.startswith("/") or any(c in link for c in "\\\n\r\0"):
                raise ValueError("Unsafe archive link")
            parts = list(PurePosixPath(name).parent.parts) if m.issym() else []
            for part in link.split("/"):
                if part == "..":
                    if not parts:
                        raise ValueError("Archive link escapes extraction root")
                    parts.pop()
                elif part not in ("", "."):
                    parts.append(part)
            return target_for("/".join(parts), visiting | {name})

        resolved = {n: target_for(n, set()) for n, m in entries.items() if not m.isdir()}
        for name, m in entries.items():
            p = output / name
            if m.isdir():
                p.mkdir(parents=True, exist_ok=True)
                continue
            source = resolved[name]
            p.parent.mkdir(parents=True, exist_ok=True)
            with t.extractfile(source) as src, p.open("xb") as dest:
                shutil.copyfileobj(src, dest)
            p.chmod(source.mode & 0o777)


def files(root):
    for path in sorted(root.rglob("*")):
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            raise ValueError(f"Resource link or special file: {path}")
        relative(path.relative_to(root).as_posix())
        if path.is_file():
            yield path


def inventory(root):
    # Recomputed after nested code signing, which changes Mach-O bytes.
    lines = [f"{digest(p)}  {p.relative_to(root).as_posix()}\n"
             for p in files(root) if p.name != "SHA256SUMS"]
    (root / "SHA256SUMS").write_text("".join(lines))


def validate(root):
    root = Path(root)
    if root.is_symlink() or not root.is_dir():
        raise ValueError("Language resources must be a real directory")
    actual = {p.relative_to(root).as_posix() for p in files(root)}
    sums = root / "SHA256SUMS"
    seen = set()
    for line in sums.read_text().splitlines():
        if not re.fullmatch(r"[a-f0-9]{64}  .+", line):
            raise ValueError("Invalid resource checksum inventory")
        name = line[66:]
        relative(name)
        if name in seen or name == "SHA256SUMS" or name not in actual:
            raise ValueError(f"Missing or duplicate resource: {name}")
        seen.add(name)
        if digest(root / name) != line[:64]:
            raise ValueError(f"Corrupt language resource: {name}")
    if actual != seen | {"SHA256SUMS"}:
        raise ValueError("Unlisted language resource")
    manifest = json.loads((root / "manifest.json").read_text())
    if manifest.get("schemaVersion") != 1 or not manifest.get("packages"):
        raise ValueError("Unsupported language resource manifest")

    def check_value(value, program=False):
        if not isinstance(value, str) or any(c in value for c in "\n\r\0"):
            raise ValueError("Invalid manifest argument")
        if value == "PATH=/usr/bin:/bin:/usr/sbin:/sbin":
            return
        path_value = value.split("=", 1)[-1]
        if path_value.startswith("/") and value != "/usr/bin/env":
            raise ValueError("Build-machine absolute path in manifest")
        if TOKEN in value:
            for part in value.split("=", 1)[-1].split(":"):
                if TOKEN in part:
                    if not part.startswith(TOKEN):
                        raise ValueError("Invalid Resources substitution")
                    name = part[len(TOKEN):]
                    relative(name)
                    if not (root / name).exists():
                        raise ValueError(f"Missing manifest resource: {name}")
                    if program and not os.access(root / name, os.X_OK):
                        raise ValueError("Resource executable is not executable")
        elif program and value != "/usr/bin/env":
            raise ValueError("Nonrelocatable executable")

    seen_packages = set()
    for tool in manifest["packages"]:
        if tool["package"] != "java" or tool["package"] in seen_packages or tool["distribution"] != "Bundled":
            raise ValueError("Only reviewed Java distribution may be bundled")
        seen_packages.add(tool["package"])
        if not tool["artifacts"] or not tool["notices"] or not tool["servers"]:
            raise ValueError("Missing provenance/notices/server")
        for artifact in tool["artifacts"]:
            if not artifact["version"] or not artifact["source_url"].startswith("https://") or not re.fullmatch(r"sha256:[a-f0-9]{64}", artifact["checksum"]):
                raise ValueError("Invalid artifact provenance")
        for command in tool["servers"] + tool["formatters"]:
            check_value(command["program"], True)
            for arg in command["args"]:
                check_value(arg)
            if command["program"] == "/usr/bin/env":
                payload = next((a for a in command["args"] if not a.startswith("-") and "=" not in a), "")
                check_value(payload, True)
                if not payload.startswith(TOKEN):
                    raise ValueError("Env payload must be bundled")
        for notice in tool["notices"]:
            if not notice.startswith(TOKEN):
                raise ValueError("Notice must be bundled")
            check_value(notice)
    for required in ("legal/NOTICE.md", "legal/provenance.json", "legal/SOURCE-ACCESS.md", "recipes/kotlin.json"):
        if required not in actual:
            raise ValueError(f"Required legal resource missing: {required}")
    return manifest


def jar_legal(jdtls, legal):
    """Preserve original JARs; make embedded legal material easy to inspect.

    Freeze the complete component set by SHA in the reviewed lockfile. New or
    changed dependencies require explicit review, never a root-license fallback.
    """
    components = []
    def inspect(name, data, depth=0):
        if depth > 4 or len(data) > 512 * 1024**2:
            raise ValueError("Excessive nested JAR")
        with zipfile.ZipFile(io.BytesIO(data)) as jar:
            text = jar.read("META-INF/MANIFEST.MF").decode(errors="replace").replace("\r\n ", "") if "META-INF/MANIFEST.MF" in jar.namelist() else ""
            headers = dict(line.split(": ", 1) for line in text.splitlines() if ": " in line)
            notices = []
            maven = []
            for entry in jar.namelist():
                if entry.endswith("/"):
                    continue
                if any(s in entry.lower() for s in ("license", "notice", "about.html", "about_files/")) and not entry.endswith(".class"):
                    relative(entry)
                    target = legal / "jdtls-jars" / name / entry
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(jar.read(entry))
                    notices.append(target.relative_to(legal).as_posix())
                if entry.endswith("/pom.properties"):
                    properties = dict(line.split("=", 1) for line in jar.read(entry).decode(errors="replace").splitlines() if "=" in line and not line.startswith("#"))
                    if all(k in properties for k in ("groupId", "artifactId", "version")):
                        g, a, v = (properties[k] for k in ("groupId", "artifactId", "version"))
                        maven.append(dict(group=g, artifact=a, version=v, sourceUrl=f"https://repo.maven.apache.org/maven2/{g.replace('.', '/')}/{a}/{v}/{a}-{v}-sources.jar"))
            components.append(dict(path=name, sha256=hashlib.sha256(data).hexdigest(), symbolicName=headers.get("Bundle-SymbolicName", ""),
                                   version=headers.get("Bundle-Version", ""), license=headers.get("Bundle-License", ""),
                                   sourceReference=headers.get("Eclipse-SourceReferences", ""), maven=maven, notices=notices))
            for entry in jar.namelist():
                if entry.endswith(".jar"):
                    relative(entry)
                    inspect(name + "!/" + entry, jar.read(entry), depth + 1)
    for p in sorted(jdtls.rglob("*.jar")):
        inspect(p.relative_to(jdtls).as_posix(), p.read_bytes())
    return components


def prepare(resources, cache, offline=False):
    lock = json.loads(LOCK.read_text())
    archives = {a["id"]: download(a, cache, offline) for a in lock["artifacts"]}
    resources.mkdir(parents=True, exist_ok=True)
    target = resources / "language-tools"
    if target.is_symlink():
        raise ValueError("Refusing resource destination symlink")
    if target.exists():
        validate(target)  # Never remove an unrelated/invalid tree.
    with tempfile.TemporaryDirectory(prefix=".language-prepare-", dir=resources) as tmp:
        stage = Path(tmp) / "language-tools"
        stage.mkdir()
        java = stage / "java"
        safe_extract(archives["jdtls"], java / "jdtls")
        unpack = Path(tmp) / "runtime"
        safe_extract(archives["temurin"], unpack)
        roots = list(unpack.iterdir())
        if len(roots) != 1 or not (roots[0] / "Contents/Home/bin/java").is_file():
            raise ValueError("Unexpected Temurin archive root")
        shutil.move(str(roots[0]), java / "runtime")
        legal = stage / "legal"
        legal.mkdir()
        components = jar_legal(java / "jdtls", legal)
        if {c["path"]: c["sha256"] for c in components} != lock["reviewedJars"]:
            raise ValueError("JDT dependencies changed: full distribution review required")
        (legal / "jdtls-components.json").write_text(json.dumps(components, indent=2) + "\n")
        for a in lock["artifacts"]:
            if a["id"] in ("temurin", "jdtls"):
                continue
            dest = stage / a["destination"]
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(archives[a["id"]], dest)
            # Preserve copyright/license headers from supplemental source JARs,
            # including transitive Maven components missing binary notices.
            if a["name"].endswith(("source.jar", "sources.jar")):
                with zipfile.ZipFile(archives[a["id"]]) as jar:
                    headers = set()
                    for n in jar.namelist():
                        if n.endswith(".java"):
                            text = jar.read(n).decode(errors="replace")
                            prefix = text.split("package ", 1)[0].strip()
                            if "copyright" in prefix.lower() or "license" in prefix.lower() or "public domain" in prefix.lower():
                                headers.add(prefix)
                    if headers:
                        out = legal / "source-copyrights" / (a["name"] + ".txt")
                        out.parent.mkdir(exist_ok=True)
                        out.write_text("\n\n---\n\n".join(sorted(headers)) + "\n")
        # Exact BSD copyright/redistribution terms from the pinned ASM sources.
        with zipfile.ZipFile(archives["asm-source"]) as jar:
            sample = jar.read("org/objectweb/asm/AnnotationVisitor.java").decode()
            (legal / "ASM-BSD-3-Clause.txt").write_text(sample.split("package org.objectweb.asm;", 1)[0])
        (legal / "provenance.json").write_text(json.dumps(lock, indent=2) + "\n")
        (legal / "NOTICE.md").write_text(Path(__file__).with_name("language-resources-NOTICE.md").read_text())
        source_lines = ["# Corresponding source and licensing access\n\n", lock["sourceStatement"], "\n\n## Exact Eclipse component source references\n\n"]
        for c in components:
            if c["sourceReference"]:
                source_lines.append(f"- `{c['path']}`: {c['sourceReference']}\n")
                match = re.search(r"https://github.com/([^;]+?)(?:\.git)?;.*commitId=([0-9a-f]{40})", c["sourceReference"])
                if match:
                    repo, commit = match.groups()
                    repo = {"eclipse/eclipse.jdt.ls": "eclipse-jdtls/eclipse.jdt.ls",
                            "eclipse.jdt.ls/eclipse.jdt.javac": "eclipse-jdtls/eclipse.jdt.javac"}.get(repo, repo)
                    source_lines.append(f"  Official source snapshot: https://github.com/{repo}/tree/{commit}\n")
            for m in c["maven"]:
                source_lines.append(f"- `{c['path']}` ({m['group']}:{m['artifact']}:{m['version']}): {m['sourceUrl']}\n")
        source_lines.append("\n## Pinned source, metadata and license artifacts\n\n")
        for a in lock["artifacts"]:
            source_lines.append(f"- {a['name']} SHA-256 `{a['sha256']}`: {a['url']}\n")
        (legal / "SOURCE-ACCESS.md").write_text("".join(source_lines))
        (stage / "recipes").mkdir()
        (stage / "recipes/kotlin.json").write_text(json.dumps(lock["kotlin"], indent=2) + "\n")
        launcher = list((java / "jdtls/plugins").glob("org.eclipse.equinox.launcher_*.jar"))
        if len(launcher) != 1:
            raise ValueError("JDT requires one Equinox launcher")
        runtime = TOKEN + "java/runtime/Contents/Home"
        args = ["-i", "PATH=/usr/bin:/bin:/usr/sbin:/sbin", "JAVA_HOME=" + runtime, "FORKARAK_MANAGED=1", runtime + "/bin/java",
                "-Declipse.application=org.eclipse.jdt.ls.core.id1", "-Dosgi.bundles.defaultStartLevel=4",
                "-Declipse.product=org.eclipse.jdt.ls.core.product", "-Dlog.level=WARNING", "-Xmx1G",
                "--add-modules=ALL-SYSTEM", "--add-opens", "java.base/java.util=ALL-UNNAMED", "--add-opens", "java.base/java.lang=ALL-UNNAMED",
                "-jar", TOKEN + "java/jdtls/plugins/" + launcher[0].name, "-configuration", TOKEN + "java/jdtls/config_mac_arm", "-data", "{workspace}"]
        artifacts = [dict(version=a["version"], source_url=a["url"], checksum="sha256:" + a["sha256"], metadata_url=a.get("metadataUrl", "")) for a in lock["artifacts"]]
        tool = dict(package="java", version="1.63.0-temurin25.0.5-8", distribution="Bundled", artifacts=artifacts,
                    servers=[dict(language="java", program="/usr/bin/env", args=args)], formatters=[],
                    notices=[TOKEN + "legal/NOTICE.md", TOKEN + "legal/provenance.json", TOKEN + "legal/SOURCE-ACCESS.md", TOKEN + "legal/jdtls-components.json"])
        (stage / "manifest.json").write_text(json.dumps(dict(schemaVersion=1, packages=[tool]), indent=2) + "\n")
        inventory(stage)
        validate(stage)
        previous = Path(tmp) / "previous"
        if target.exists():
            target.rename(previous)
        try:
            stage.rename(target)
        except BaseException:
            if previous.exists():
                previous.rename(target)
            raise
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--resources", type=Path, help="App Contents/Resources destination")
    parser.add_argument("--cache", type=Path, default=Path.home() / "Library/Caches/Forkarak/language-resources")
    parser.add_argument("--offline", action="store_true", help="Require all pinned bytes in verified cache")
    parser.add_argument("--verify", type=Path, help="Verify a prepared language-tools directory")
    parser.add_argument("--inventory", type=Path, help="Refresh SHA256SUMS after nested code signing")
    args = parser.parse_args()
    try:
        if args.verify:
            validate(args.verify)
            print(f"Verified language resources: {args.verify}")
        elif args.inventory:
            inventory(args.inventory)
            validate(args.inventory)
        elif args.resources:
            print(f"Prepared language resources: {prepare(args.resources, args.cache, args.offline)}")
        else:
            parser.error("Specify --resources, --verify or --inventory")
    except (ValueError, OSError, subprocess.CalledProcessError, tarfile.TarError, zipfile.BadZipFile, KeyError) as error:
        parser.exit(1, f"Language resource preparation failed: {error}\n")


if __name__ == "__main__":
    main()
