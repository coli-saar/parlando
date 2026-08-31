#!/usr/bin/env python3
"""Check coordinated Parlando release metadata without changing the repository."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path


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


def check_primary_packages(root: Path, version: str, errors: list[str]) -> None:
    """Check the publishable Rust and JavaScript package versions and npm lock root."""
    cargo_path = root / "rust-server/Cargo.toml"
    cargo = read_toml(cargo_path)
    package = cargo.get("package")
    if not isinstance(package, dict):
        errors.append(f"{cargo_path}: missing [package]")
    else:
        record_equal(errors, str(cargo_path), package_version(package, cargo_path), version)

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

    expected = f"^{version}"
    for path in sorted(root.rglob("package.json")):
        if any(part in {"target", "node_modules"} for part in path.parts):
            continue
        document = read_json(path)
        for section_name in ("dependencies", "devDependencies", "peerDependencies"):
            section = document.get(section_name)
            if not isinstance(section, dict) or "@coli-saar/parlando-client" not in section:
                continue
            record_equal(
                errors,
                f"{path} {section_name}.@coli-saar/parlando-client",
                section["@coli-saar/parlando-client"],
                expected,
            )


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


def check_runtime_stress_documentation(root: Path, errors: list[str]) -> None:
    """Reject obsolete stress-runner flags and require the current headless CLI surface."""
    path = root / "docs/runtime-stress-testing.md"
    text = path.read_text(encoding="utf-8")
    obsolete_flags = ("--preset", "--no-tui", "--report", "--keep-database")
    for flag in obsolete_flags:
        if flag in text:
            errors.append(f"{path}: contains obsolete runtime-stress flag {flag!r}")
    for flag in ("--pairing", "--sessions", "--seconds", "--headless", "--output"):
        if flag not in text:
            errors.append(f"{path}: missing current runtime-stress flag {flag!r}")


def check_release_documents(root: Path, version: str, errors: list[str]) -> None:
    """Check current release guidance, changelog metadata, and the generator skill baseline."""
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
    marker = f"Current coordinated release: `{version}`."
    if marker not in generator:
        errors.append(f"{generator_path}: missing release marker {marker!r}")
    if "docs/migrating-to-clean-api.md" in generator:
        errors.append(f"{generator_path}: still references obsolete migration guide")

    changelog_path = root / "CHANGELOG.md"
    changelog = changelog_path.read_text(encoding="utf-8")
    if not re.search(rf"^## \[{re.escape(version)}\] - \d{{4}}-\d{{2}}-\d{{2}}$", changelog, re.MULTILINE):
        errors.append(f"{changelog_path}: missing dated [{version}] release heading")


def check_migration_guide(root: Path, version: str, errors: list[str]) -> None:
    """Check that one target migration guide covers the required release-risk categories."""
    candidates = sorted((root / "docs").glob(f"migrating-*-to-{version}.md"))
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
    parser.add_argument("version", help="target stable SemVer, for example 0.4.0")
    parser.add_argument(
        "--published",
        action="store_true",
        help="also require registry-derived first-party npm consumer lock entries",
    )
    args = parser.parse_args()
    if not re.fullmatch(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", args.version):
        parser.error("version must be stable SemVer in MAJOR.MINOR.PATCH form")
    return args


def main() -> int:
    """Run all non-mutating release metadata checks and return a shell-friendly status."""
    args = parse_args()
    root = repository_root()
    errors: list[str] = []
    try:
        check_primary_packages(root, args.version, errors)
        check_consumers(root, args.version, errors)
        if args.published:
            check_published_consumer_lockfiles(root, args.version, errors)
        check_release_documents(root, args.version, errors)
        check_migration_guide(root, args.version, errors)
        check_runtime_stress_documentation(root, errors)
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        errors.append(str(error))

    if errors:
        print(f"release metadata check failed for {args.version}:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    suffix = " with published consumer locks" if args.published else ""
    print(f"release metadata check passed for {args.version}{suffix}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
