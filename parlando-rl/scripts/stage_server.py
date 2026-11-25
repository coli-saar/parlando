"""Prepare a standalone OpenEnv build context from a Parlando source checkout."""

import argparse
import subprocess
from pathlib import Path
from shutil import copy2, copytree, ignore_patterns

import yaml

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 with the OpenEnv extra.
    import tomli as tomllib

PACKAGE = Path(__file__).resolve().parents[1]
REPO = PACKAGE.parent
# Copy only build inputs. A checkout may also contain credentials, local databases
# and large Cargo targets, none of which a deployable environment needs.
VENDOR_INPUTS = (
    "README.md",
    "rust-server/Cargo.toml", "rust-server/build.rs", "rust-server/src",
    "rust-server/wit", "rust-server/proto", "rust-server/brand",
    "rust-server/config/runtime-limits.json",
    "parlando-game-sdk/Cargo.toml", "parlando-game-sdk/src",
    "parlando-game-sdk/wit", "parlando-game-sdk/examples",
    "parlando-game-sdk/derive/Cargo.toml", "parlando-game-sdk/derive/src",
    "parlando-agent-sdk/README.md", "parlando-agent-sdk/pyproject.toml",
    "parlando-agent-sdk/src/parlando_agent_sdk",
)
ENV_INPUTS = (
    "README.md", "Cargo.toml", "Cargo.lock", "src", "pyproject.toml",
    "python", "uv.lock", "openenv.yaml", "server", "__init__.py", "client.py", "models.py",
)


def copy_inputs(source: Path, destination: Path, entries: tuple[str, ...]) -> None:
    """Preserve the relative paths used by Cargo, maturin, and Python imports."""
    for entry in entries:
        src, dst = source / entry, destination / entry
        dst.parent.mkdir(parents=True, exist_ok=True)
        if src.is_dir():
            copytree(src, dst, ignore=ignore_patterns("__pycache__", "*.pyc", "*.so", ".DS_Store"))
        else:
            copy2(src, dst)


def stage(config: Path, destination: Path) -> None:
    """Collect local dependencies, the chosen game, and any configured WASM opponent."""
    settings = yaml.safe_load(config.read_text(encoding="utf-8"))
    game = config.parent / settings["game"]
    opponent = settings["opponent"]
    packages = [(game, "game.zip")]
    settings["game"] = "game.zip"
    if "path" in opponent:
        packages.append((config.parent / opponent["path"], "opponent.zip"))
        opponent["path"] = "opponent.zip"
    for package, _ in packages:
        if not package.is_file():
            raise FileNotFoundError(package)
    destination.mkdir(parents=True, exist_ok=False)
    copy_inputs(PACKAGE, destination, ENV_INPUTS)
    copy_inputs(REPO, destination / "vendor", VENDOR_INPUTS)
    # The checkout resolves sibling packages. The standalone distribution
    # carries the same sources and lockfile identities inside its build context.
    for filename in ("Cargo.toml", "pyproject.toml", "uv.lock"):
        manifest = destination / filename
        text = manifest.read_text(encoding="utf-8")
        for dependency in ("rust-server", "parlando-agent-sdk"):
            text = text.replace(f'"../{dependency}"', f'"vendor/{dependency}"')
        manifest.write_text(text, encoding="utf-8")
    # OpenEnv validates the server's mandatory dependencies. The standalone
    # server always uses the adapter that is optional in the general RL wheel.
    manifest = tomllib.loads((destination / "pyproject.toml").read_text(encoding="utf-8"))
    requirements = manifest["project"]["optional-dependencies"]["openenv"]
    subprocess.run(["uv", "add", "--no-sync", *requirements], cwd=destination, check=True)
    deployment = destination / "deployment"
    deployment.mkdir()
    for package, filename in packages:
        copy2(package, deployment / filename)
    (deployment / "environment.yaml").write_text(yaml.safe_dump(settings), encoding="utf-8")


def main() -> None:
    """Stage source and configuration into a new directory without publishing it."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path, help="server environment YAML")
    parser.add_argument("destination", type=Path, help="new build context directory")
    args = parser.parse_args()
    stage(args.config, args.destination)


if __name__ == "__main__":
    main()
