"""Import the file artwork and associations from the pinned upstream release VSIX."""
import hashlib
import json
from pathlib import Path
import sys
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parents[1] / "assets/icons/vscode"
SHA256 = "0f38ea41a7a9395d79d2ca712b5930009416fa609b9a77f9eb90247125258983"

# Standalone clients do not have VS Code language detection. Preserve Pokarak's
# build-file conventions and resolve common language-only associations by name.
NAMES = {
    "cargo.toml": "_f_cargo", "cargo.lock": "_f_cargo",
    "gradlew": "_f_gradle", "gradlew.bat": "_f_gradle",
    "gradle.properties": "_f_gradle", "gradle-wrapper.properties": "_f_gradle",
    "dockerfile": "_f_docker", "containerfile": "_f_docker",
    "makefile": "_f_gnu", "gnumakefile": "_f_gnu", "readme": "_f_markdown",
}
SUFFIXES = {"kts": "_f_kotlin", "htm": "_f_html", "css": "_f_css"}


def main(package):
    if hashlib.sha256(package.read_bytes()).hexdigest() != SHA256:
        raise SystemExit("Expected the official vscode-icons v13.0.0 release VSIX")
    with ZipFile(package) as archive:
        def theme(name):
            return json.loads(archive.read(f"extension/dist/src/{name}.json"))

        manifest = theme("vsicons-icon-theme")
        # Upstream's standalone theme supplies suffixes such as rs, tsx and py
        # which VS Code itself resolves through installed language providers.
        standalone = theme("vsicons-icon-theme-zed")["themes"][0]
        definitions = manifest["iconDefinitions"]
        names = {**standalone["file_stems"], **NAMES, **manifest["fileNames"]}
        suffixes = {**standalone["file_suffixes"], **SUFFIXES, **manifest["fileExtensions"]}
        light_theme = manifest["light"]
        light_ids = {}
        for key in ("fileNames", "fileExtensions", "languageIds"):
            for name, dark_id in manifest[key].items():
                light_id = light_theme[key].get(name, dark_id)
                if light_id != dark_id:
                    light_ids[dark_id] = light_id
        # This release leaves the optional light default empty; inherit the base artwork.
        if definitions[light_theme["file"]]["iconPath"]:
            light_ids[manifest["file"]] = light_theme["file"]

        def filename(icon):
            return Path(definitions[icon]["iconPath"]).name

        ids = set(names.values()) | set(suffixes.values()) | {manifest["file"]}
        variants = {filename(i): filename(light_ids[i]) for i in ids if i in light_ids}
        assets = {filename(i) for i in ids} | set(variants.values())
        artwork = ROOT / "artwork"
        artwork.mkdir(parents=True, exist_ok=True)
        for name in sorted(assets):
            (artwork / name).write_bytes(archive.read(f"extension/icons/{name}"))
        for stale in artwork.glob("*.svg"):
            if stale.name not in assets:
                stale.unlink()

        data = {
            "default": filename(manifest["file"]),
            "names": {k.lower(): filename(v) for k, v in names.items()},
            "suffixes": {k.lower(): filename(v) for k, v in suffixes.items()},
            "light": variants,
        }
        (ROOT / "associations.json").write_text(json.dumps(data, sort_keys=True, indent=2) + "\n")
        (ROOT / "LICENSE.txt").write_bytes(archive.read("extension/LICENSE.txt"))
        print(f"{len(assets)} SVGs, {len(data['names'])} filenames, {len(data['suffixes'])} suffixes")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
