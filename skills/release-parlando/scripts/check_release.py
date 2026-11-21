#!/usr/bin/env python3
"""Check coordinated Parlando release metadata without changing the repository."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path

CLIENT_TEMPLATE = Path("rust-server/templates/rust/client/package.json")


def repository_root() -> Path:
    """Return the Parlando repository root relative to this bundled skill script."""
    return Path(__file__).resolve().parents[3]


def read_json(path: Path) -> dict[str, object]:
    """Load one JSON object and fail with a useful path when its shape is invalid."""
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path}: expected a JSON object")
    return value


def read_toml(path: Path) -> dict[str, object]:
    """Load one TOML document as a mapping."""
    with path.open("rb") as stream:
        return tomllib.load(stream)


def package_version(package: dict[str, object], path: Path) -> str:
    """Return a package version from a parsed manifest or raise a precise error."""
    value = package.get("version")
    if not isinstance(value, str):
        raise ValueError(f"{path}: package version is missing or not a string")
    return value


def record_equal(errors: list[str], label: str, actual: object, expected: object) -> None:
    """Append one diagnostic when a release value differs from its expected value."""
    if actual != expected:
        errors.append(f"{label}: expected {expected!r}, found {actual!r}")


def check_release_packages(root: Path, version: str, errors: list[str]) -> None:
    """Check the six release versions, exact Rust SDK pins, and npm lock root."""
    manifests = {
        "rust-server/Cargo.toml": "package",
        "parlando-game-sdk/Cargo.toml": "package",
        "parlando-game-sdk/derive/Cargo.toml": "package",
        "parlando-python-sdk/pyproject.toml": "project",
        "parlando-agent-sdk/pyproject.toml": "project",
    }
    for filename, section in manifests.items():
        path = root / filename
        package = read_toml(path).get(section)
        if not isinstance(package, dict):
            errors.append(f"{path}: missing [{section}]")
        else:
            record_equal(errors, str(path), package_version(package, path), version)

    for filename, name in (
        ("rust-server/Cargo.toml", "parlando-game-sdk"),
        ("parlando-game-sdk/Cargo.toml", "parlando-game-derive"),
    ):
        path = root / filename
        dependency = read_toml(path).get("dependencies", {}).get(name)
        record_equal(
            errors, f"{path} dependency {name}", direct_parlando_version(dependency), f"={version}"
        )

    package_path = root / "js-client/package.json"
    package_json = read_json(package_path)
    record_equal(errors, str(package_path), package_json.get("version"), version)

    lock_path = root / "js-client/package-lock.json"
    lock = read_json(lock_path)
    record_equal(errors, f"{lock_path} root", lock.get("version"), version)
    packages = lock.get("packages")
    lock_root = packages.get("") if isinstance(packages, dict) else None
    lock_version = lock_root.get("version") if isinstance(lock_root, dict) else None
    record_equal(errors, f"{lock_path} packages['']", lock_version, version)


def direct_parlando_version(value: object) -> str | None:
    """Extract a registry version from a Cargo dependency declaration, if present."""
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        candidate = value.get("version")
        return candidate if isinstance(candidate, str) else None
    return None


def check_consumers(root: Path, version: str, errors: list[str]) -> None:
    """Check all first-party manifests that directly consume either Parlando package."""
    for path in sorted(root.rglob("Cargo.toml")):
        if any(part in {"target", "node_modules"} for part in path.parts):
            continue
        document = read_toml(path)
        for section_name in ("dependencies", "dev-dependencies", "build-dependencies"):
            section = document.get(section_name)
            if not isinstance(section, dict) or "parlando" not in section:
                continue
            declared = direct_parlando_version(section["parlando"])
            if declared is not None:
                record_equal(errors, f"{path} [{section_name}].parlando", declared, version)

    for path in sorted(root.rglob("package.json")):
        if any(part in {"target", "node_modules"} for part in path.parts):
            continue
        if path == root / "client-server-tests/browser/package.json":
            # This unpublished fixture deliberately consumes the local package under test.
            continue
        document = read_json(path)
        for section_name in ("dependencies", "devDependencies", "peerDependencies"):
            section = document.get(section_name)
            if not isinstance(section, dict) or "@coli-saar/parlando-client" not in section:
                continue
            declared = section["@coli-saar/parlando-client"]
            if path == root / CLIENT_TEMPLATE:
                record_equal(errors, str(path), declared, "__CLIENT_VERSION__")
            elif declared not in (version, f"^{version}"):
                errors.append(f"{path} {section_name}: expected client version {version}, found {declared!r}")


def check_published_consumer_lockfiles(root: Path, version: str, errors: list[str]) -> None:
    """Check registry-derived npm lock entries after the coordinated package is published."""
    package_name = "@coli-saar/parlando-client"
    lock_key = f"node_modules/{package_name}"
    expected_tarball = (
        "https://registry.npmjs.org/@coli-saar/parlando-client/-/"
        f"parlando-client-{version}.tgz"
    )
    for package_path in sorted(root.rglob("package.json")):
        if any(part in {"target", "node_modules"} for part in package_path.parts):
            continue
        if package_path == root / "client-server-tests/browser/package.json":
            # The unpublished browser fixture must exercise the local package under test.
            continue
        if package_path == root / CLIENT_TEMPLATE:
            # The CLI substitutes its release version when creating the project.
            continue
        package = read_json(package_path)
        dependencies = package.get("dependencies")
        if not isinstance(dependencies, dict) or package_name not in dependencies:
            continue
        lock_path = package_path.with_name("package-lock.json")
        if not lock_path.exists():
            errors.append(f"{package_path}: published consumer has no package-lock.json")
            continue
        lock = read_json(lock_path)
        packages = lock.get("packages")
        entry = packages.get(lock_key) if isinstance(packages, dict) else None
        if not isinstance(entry, dict):
            errors.append(f"{lock_path}: missing {lock_key!r} registry entry")
            continue
        record_equal(errors, f"{lock_path} {lock_key} version", entry.get("version"), version)
        record_equal(
            errors,
            f"{lock_path} {lock_key} resolved",
            entry.get("resolved"),
            expected_tarball,
        )
        integrity = entry.get("integrity")
        if not isinstance(integrity, str) or not integrity.startswith("sha512-"):
            errors.append(f"{lock_path} {lock_key}: missing registry sha512 integrity")


def check_release_documents(root: Path, version: str, errors: list[str]) -> None:
    """Check current release guidance and changelog metadata."""
    publishing_path = root / "docs/publishing-packages.md"
    publishing = publishing_path.read_text(encoding="utf-8")
    rust_examples = re.findall(r'^parlando = "([0-9]+\.[0-9]+\.[0-9]+)"$', publishing, re.MULTILINE)
    npm_examples = re.findall(
        r'^"@coli-saar/parlando-client": "\^([0-9]+\.[0-9]+\.[0-9]+)"$',
        publishing,
        re.MULTILINE,
    )
    if not rust_examples or any(item != version for item in rust_examples):
        errors.append(f"{publishing_path}: current Rust examples must all use {version}")
    if not npm_examples or any(item != version for item in npm_examples):
        errors.append(f"{publishing_path}: current npm examples must all use ^{version}")

    generator_path = root / "skills/generate-parlando-game/SKILL.md"
    generator = generator_path.read_text(encoding="utf-8")
    if "docs/migrating-to-clean-api.md" in generator:
        errors.append(f"{generator_path}: still references obsolete migration guide")

    changelog_path = root / "CHANGELOG.md"
    changelog = changelog_path.read_text(encoding="utf-8")
    if not re.search(rf"^## \[{re.escape(version)}\] - \d{{4}}-\d{{2}}-\d{{2}}$", changelog, re.MULTILINE):
        errors.append(f"{changelog_path}: missing dated [{version}] release heading")


def check_migration_guide(
    root: Path, version: str, required: bool, errors: list[str]
) -> None:
    """Validate a target migration guide when present or required by downstream changes."""
    candidates = sorted((root / "docs").glob(f"migrating-*-to-{version}.md"))
    if not candidates and not required:
        return
    if len(candidates) != 1:
        errors.append(f"docs: expected exactly one migration guide ending in -to-{version}.md")
        return
    text = candidates[0].read_text(encoding="utf-8").lower()
    required_concepts = {
        "Rust dependency bump": f'parlando = "{version}"'.lower(),
        "JavaScript dependency bump": f'"@coli-saar/parlando-client": "^{version}"'.lower(),
        "database backup": "backup",
        "database conversion": "sqlite",
        "Rust API migration": "gamefactory",
        "JavaScript API migration": "typescript",
        "configuration migration": "configuration",
        "validation checklist": "validate the upgrade",
    }
    for label, needle in required_concepts.items():
        if needle not in text:
            errors.append(f"{candidates[0]}: missing {label} coverage ({needle!r})")


def parse_args() -> argparse.Namespace:
    """Parse and validate the target stable semantic version from the command line."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", nargs="?", help="target stable SemVer, for example 0.4.0")
    parser.add_argument(
        "--packages-only",
        action="store_true",
        help="check package metadata only and default to the server crate version",
    )
    parser.add_argument(
        "--published",
        action="store_true",
        help="also require registry-derived first-party npm consumer lock entries",
    )
    parser.add_argument(
        "--require-migration-guide",
        action="store_true",
        help="require and validate a version-specific downstream migration guide",
    )
    args = parser.parse_args()
    if not args.packages_only and args.version is None:
        parser.error("version is required for a full release audit")
    if args.packages_only and (args.published or args.require_migration_guide):
        parser.error("--packages-only requires a package metadata check without additional flags")
    if args.version is not None and not re.fullmatch(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", args.version):
        parser.error("version must be stable SemVer in MAJOR.MINOR.PATCH form")
    return args


def main() -> int:
    """Run all non-mutating release metadata checks and return a shell-friendly status."""
    args = parse_args()
    root = repository_root()
    errors: list[str] = []
    version = args.version
    try:
        if version is None:
            path = root / "rust-server/Cargo.toml"
            version = package_version(read_toml(path).get("package", {}), path)
        check_release_packages(root, version, errors)
        if not args.packages_only:
            check_consumers(root, version, errors)
            if args.published:
                check_published_consumer_lockfiles(root, version, errors)
            check_release_documents(root, version, errors)
            check_migration_guide(root, version, args.require_migration_guide, errors)
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        errors.append(str(error))

    if errors:
        print(f"release metadata check failed for {version}:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    suffix = " with published consumer locks" if args.published else ""
    print(f"release metadata check passed for {version}{suffix}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
