"""Build a game's packages and deploy the verified output to an existing Parlando host."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def digest(path):
    """Identify the exact archive without loading the browser or Wasm assets into memory."""
    checksum = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def build(root, cli, game_only):
    """Pin each agent to the game just built and record the complete ordered upload set."""
    def archive(manifest):
        """Use the CLI's reported output path for this manifest and release."""
        output = subprocess.check_output([*cli, "build", "--manifest", manifest, "--json"])
        report = json.loads(output)
        path = Path(report["result"]["package"]).resolve()
        print(f"Built {path}", flush=True)
        return path

    game = archive(root / "game.yaml")
    packages = [game]
    if not game_only:
        agents = [root / "agent.yaml", *sorted(root.glob("agents/*/agent.yaml"))]
        for manifest in agents:
            if manifest.is_file():
                subprocess.run([*cli, "agent", "set-game", game, "--manifest", manifest], check=True)
                packages.append(archive(manifest))
    receipt = [{"path": str(path.relative_to(root)), "sha256": digest(path)} for path in packages]
    with tempfile.NamedTemporaryFile(mode="w", dir=root / "dist", delete=False) as output:
        json.dump(receipt, output, indent=2)
        output.write("\n")
    os.replace(output.name, root / "dist/deploy-packages.json")


def deploy(root, cli, host):
    """Verify every archive before uploading the game followed by its agents."""
    receipt = root / "dist/deploy-packages.json"
    if not receipt.is_file():
        raise ValueError(f"Build the packages with {root / 'build.sh'} before deploying.")
    packages = []
    for entry in json.loads(receipt.read_text()):
        path = root / entry["path"]
        if not path.is_file() or digest(path) != entry["sha256"]:
            raise ValueError(f"Missing or changed archive {path}. Run {root / 'build.sh'} again.")
        packages.append(path)
    if not packages:
        raise ValueError(f"Empty package list in {receipt}. Run {root / 'build.sh'} again.")
    target = ["--host", host] if host else []
    for path in packages:
        subprocess.run([*cli, "deploy", *target, path], check=True)


def main():
    """Select the checkout CLI or PARLANDO_BIN and dispatch the project's entry point."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("project", type=Path)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("build").add_argument("--game-only", action="store_true",
                                              help="Build and record only the game ZIP.")
    commands.add_parser("deploy").add_argument("--host", metavar="URL_OR_ALIAS",
                                               help="Target server (defaults to the CLI's default alias).")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    root = args.project.resolve()
    cli = [os.environ["PARLANDO_BIN"]] if os.environ.get("PARLANDO_BIN") else [
        "cargo", "run", "--quiet", "--manifest-path", str(repo / "rust-server/Cargo.toml"),
        "--bin", "parlando", "--",
    ]
    output = root / "dist"
    if args.command == "build":
        output.mkdir(exist_ok=True)
    elif not output.is_dir():
        raise ValueError(f"Run {root / 'build.sh'} before deploying.")

    # Serialize multi-package builds and hold a stable release throughout an upload.
    with (output / ".example-packages.lock").open("a") as lock:
        if args.command == "build":
            fcntl.flock(lock, fcntl.LOCK_EX)
            os.environ.setdefault("npm_config_cache", os.getenv("NPM_CACHE", str(repo / ".local/npm-cache")))
            build(root, cli, args.game_only)
        else:
            fcntl.flock(lock, fcntl.LOCK_SH)
            deploy(root, cli, args.host)


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        if error.output:
            print(error.output.decode(), file=sys.stderr)
        sys.exit(error.returncode)
    except (OSError, ValueError, KeyError) as error:
        sys.exit(str(error))
