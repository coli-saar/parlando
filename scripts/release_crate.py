"""Run a Cargo publication check without uploading a crate."""

import argparse
import json
from pathlib import Path
import subprocess
import tomllib
from urllib.error import HTTPError
from urllib.request import urlopen


def indexed(name: str, version: str) -> bool:
    """Check the public Cargo index for the exact usable version."""
    url = f"https://index.crates.io/{name[:2]}/{name[2:4]}/{name}"
    try:
        with urlopen(url, timeout=30) as response:
            entries = [json.loads(line) for line in response if line.strip()]
    except HTTPError as error:
        if error.code == 404:
            return False
        raise
    for entry in entries:
        if entry["vers"] == version:
            if entry["yanked"]:
                raise SystemExit(f"{name} {version} is yanked from crates.io")
            return True
    return False


def check(manifest: Path) -> None:
    """List package contents when a dependency's new version is still local."""
    package = tomllib.loads(manifest.read_text(encoding="utf-8"))
    name = package["package"]["name"]
    options = ["publish", "--registry", "crates-io", "--dry-run"]
    for dependency in package.get("dependencies", {}).values():
        if not isinstance(dependency, dict) or "path" not in dependency:
            continue
        local = manifest.parent / dependency["path"] / "Cargo.toml"
        required = tomllib.loads(local.read_text(encoding="utf-8"))["package"]
        if not indexed(required["name"], required["version"]):
            print(
                f"{name}: listing package contents until "
                f"{required['name']} {required['version']} is indexed",
                flush=True,
            )
            options = ["package", "--list"]
            break
    options.append("--allow-dirty")
    result = subprocess.run(["cargo", *options], cwd=manifest.parent)
    if result.returncode:
        raise SystemExit(result.returncode)


def main() -> None:
    """Run the selected Cargo release check for one crate."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    args = parser.parse_args()
    check(args.manifest.resolve())


if __name__ == "__main__":
    main()
