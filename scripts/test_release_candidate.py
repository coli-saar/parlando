"""Rehearse a coordinated release with locally packaged artifacts."""

import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
from urllib.error import URLError
from urllib.request import ProxyHandler, build_opener


ROOT = Path(__file__).resolve().parents[1]
CLIENT = "@coli-saar/parlando-client"
CRATES = (
    ("parlando-game-derive", "parlando-game-sdk/derive"),
    ("parlando-game-sdk", "parlando-game-sdk"),
    ("parlando", "rust-server"),
)
PYTHON_SDKS = (
    ("parlando-python-sdk", "parlando_game_sdk"),
    ("parlando-agent-sdk", "parlando_agent_sdk"),
)
GAMES = {"rust": "release-rust", "python": "release-python"}
JS_GENERATED = ("node_modules", "dist", "coverage", ".local")
PYTHON_GENERATED = (".venv", "dist", "build", "*.egg-info", "__pycache__", ".pytest_cache", ".benchmarks")


def run(*args, cwd, env, capture=False, timeout=None):
    """Run one release step with live diagnostics or capture its structured output."""
    print(f"[{cwd.name}] {args[0]} {' '.join(map(str, args[1:]))}", file=sys.stderr, flush=True)
    result = subprocess.run(
        args, cwd=cwd, env=env, text=True, timeout=timeout,
        stdout=subprocess.PIPE if capture else None,
    )
    if result.returncode and result.stdout:
        print(result.stdout, file=sys.stderr, end="")
    result.check_returncode()
    return result.stdout


def package_metadata(path):
    """Read the package section used by the Cargo release steps."""
    with path.open("rb") as manifest:
        return tomllib.load(manifest)["package"]


def cargo_inputs(source, env):
    """Find the checkout files selected by Cargo, including the input lockfile and README."""
    original = ROOT / source
    files = run(
        "cargo", "package", "--list", "--exclude-lockfile", "--allow-dirty",
        "--manifest-path", original / "Cargo.toml", cwd=ROOT, env=env, capture=True,
    ).splitlines()
    generated = {".cargo_vcs_info.json", "Cargo.toml.orig"}
    readme = package_metadata(original / "Cargo.toml").get("readme")
    paths = set(files) - generated
    if isinstance(readme, str) and Path(readme).name != readme:
        paths.discard(Path(readme).name)
        paths.add(readme)
    if (original / "Cargo.lock").is_file():
        paths.add("Cargo.lock")
    return sorted(paths)


def stage_crate(work, source, env):
    """Copy Cargo's selected inputs, so resolving unpublished crates cannot rewrite checkout locks."""
    original = ROOT / source
    stage = work / "sources" / source
    for relative in cargo_inputs(source, env):
        destination = stage / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(original / relative, destination)
    return stage


def stage_packages(work, env):
    """Freeze all six package trees before building any release artifacts."""
    for _, source in CRATES:
        stage_crate(work, source, env)
    trees = [("js-client", JS_GENERATED)]
    trees.extend((name, PYTHON_GENERATED) for name, _ in PYTHON_SDKS)
    for source, generated in trees:
        shutil.copytree(
            ROOT / source, work / "sources" / source,
            ignore=shutil.ignore_patterns(*generated, ".DS_Store"),
        )


def digest(path):
    """Hash an input or package in binary mode."""
    with path.open("rb") as data:
        return hashlib.file_digest(data, "sha256").hexdigest()


def release_sources(env):
    """Fingerprint package inputs so a stale rehearsal cannot authorize publication."""
    files = {ROOT / ".cargo/config.toml"}
    for _, source in CRATES:
        files.update((ROOT / source / relative).resolve() for relative in cargo_inputs(source, env))
    package_trees = [("js-client", JS_GENERATED), *((name, PYTHON_GENERATED) for name, _ in PYTHON_SDKS)]
    for source, generated in package_trees:
        ignore = shutil.ignore_patterns(*generated, ".DS_Store")
        for directory, subdirs, names in os.walk(ROOT / source):
            ignored = ignore(directory, subdirs + names)
            subdirs[:] = [name for name in subdirs if name not in ignored]
            files.update(Path(directory) / name for name in names if name not in ignored)
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(files)}


def check_sources(expected, env):
    """Report each changed input before accepting a recorded candidate."""
    current = release_sources(env)
    changed = sorted(path for path in current.keys() | expected.keys() if current.get(path) != expected.get(path))
    if changed:
        paths = "\n".join(f"  {path}" for path in changed)
        raise SystemExit(f"Release inputs changed. Run make test-release-candidate.\n{paths}")


def save_verified(output, artifacts, version, sources):
    """Retain artifacts from a successful run and atomically select them for publication."""
    checksums = {path.name: digest(path) for path in sorted(artifacts.iterdir())}
    output.mkdir(parents=True, exist_ok=True)
    bundle = Path(tempfile.mkdtemp(prefix=f"{version}-", dir=output))
    shutil.copytree(artifacts, bundle, dirs_exist_ok=True)
    manifest = {
        "version": version,
        "bundle": bundle.name,
        "artifacts": checksums,
        "sources": sources,
    }
    marker = bundle / "verified.json"
    marker.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    marker.replace(output / "verified.json")
    print(f"Verified release artifacts saved to {bundle}")


def verified_bundle(output, version, env):
    """Check that the current release still matches the passing candidate and return its files."""
    marker = output / "verified.json"
    if not marker.is_file():
        raise SystemExit("No verified release candidate. Run make test-release-candidate.")
    manifest = json.loads(marker.read_text(encoding="utf-8"))
    if manifest["version"] != version:
        raise SystemExit(f"No verified candidate for {version}. Run make test-release-candidate.")
    bundle = output / manifest["bundle"]
    if not bundle.is_dir():
        raise SystemExit("Verified artifacts are missing. Run make test-release-candidate.")
    artifacts = {path.name: digest(path) for path in sorted(bundle.iterdir())}
    if artifacts != manifest["artifacts"]:
        raise SystemExit("Verified artifact files changed. Run make test-release-candidate.")
    check_sources(manifest["sources"], env)
    return bundle


def install_rust(work, artifacts, version, env):
    """Package crates in dependency order and install the CLI from its extracted archive."""
    patches = []
    for name, source in CRATES:
        stage = work / "sources" / source
        run(
            "cargo", "package", "--manifest-path", stage / "Cargo.toml",
            "--allow-dirty", "--no-verify", *patches, cwd=work, env=env,
        )
        archive = f"{name}-{version}"
        shutil.copyfile(work / "cargo-target/package" / f"{archive}.crate",
                        artifacts / f"{archive}.crate")
        with tarfile.open(artifacts / f"{archive}.crate", "r:gz") as package:
            package.extractall(work / "crates", filter="data")
        extracted = work / "crates" / archive
        if name != "parlando":
            patches += ["--config", f"patch.crates-io.{name}.path={json.dumps(str(extracted))}"]

    run(
        "cargo", "install", "--path", extracted, "--bin", "parlando",
        "--locked", "--root", work / "install", *patches, cwd=work, env=env,
    )
    binary = work / "install/bin/parlando"
    if run(binary, "--version", cwd=work, env=env, capture=True).strip() != f"parlando {version}":
        raise RuntimeError(f"Installed CLI did not report version {version}")

    # The scaffold keeps its exact registry dependency. Cargo resolves it from the two archives.
    cargo_config = work / ".cargo/config.toml"
    cargo_config.parent.mkdir()
    cargo_config.write_text(
        "[patch.crates-io]\n"
        + "".join(f"{entry.removeprefix('patch.crates-io.')}\n" for entry in patches[1::2]),
        encoding="utf-8",
    )
    return binary, extracted


def pack_client(work, artifacts, env):
    """Create the npm tarball using its normal prepack build in a clean staging directory."""
    source = work / "sources/js-client"
    run("npm", "ci", "--no-audit", "--no-fund", cwd=source, env=env)
    package = json.loads(run(
        "npm", "pack", "--json", "--pack-destination", artifacts,
        cwd=source, env=env, capture=True,
    ))[0]
    return artifacts / package["filename"]


def install_python(work, artifacts, packaged_server, version, env):
    """Build both distributions, install the two wheels, and exercise the installed SDKs."""
    for name, _ in PYTHON_SDKS:
        source = work / "sources" / name
        run(sys.executable, "-m", "build", "--outdir", artifacts, source, cwd=work, env=env)
    wheels = sorted(artifacts.glob("*.whl"))
    run(
        sys.executable, "-m", "twine", "check", "--strict", *wheels,
        *sorted(artifacts.glob("*.tar.gz")), cwd=work, env=env,
    )
    venv = work / "python"
    run(sys.executable, "-m", "venv", venv, cwd=work, env=env)
    python = venv / "bin/python"
    run(
        python, "-m", "pip", "install", *wheels, "componentize-py==0.25.1",
        cwd=work, env=env,
    )
    imports = "\n".join(
        f"import {module}\nassert metadata.version({name!r}) == {version!r}"
        for name, module in PYTHON_SDKS
    )
    run(python, "-I", "-c", f"from importlib import metadata\n{imports}", cwd=work, env=env)
    run(python, "-m", "pip", "check", cwd=work, env=env)

    # The SDK tests run without their source trees. The agent protocol check uses the packaged proto.
    shutil.copytree(packaged_server / "proto", work / "rust-server/proto")
    for name, _ in PYTHON_SDKS:
        tests = work / name / "tests"
        shutil.copytree(work / "sources" / name / "tests", tests, ignore=shutil.ignore_patterns("__pycache__"))
        run(python, "-I", "-m", "unittest", "discover", "-s", tests, cwd=work, env=env)
    return venv


def build_games(work, binary, client, version, env):
    """Scaffold and build both starter games against candidate SDKs and browser assets."""
    for language, game in GAMES.items():
        run(binary, "new", game, "--lang", language, cwd=work, env=env)
        project = work / game
        client_manifest = project / "client/package.json"
        package = json.loads(client_manifest.read_text(encoding="utf-8"))
        if package["dependencies"][CLIENT] != version:
            raise RuntimeError("Starter client did not request the coordinated version")
        package["dependencies"][CLIENT] = f"file:{client}"
        client_manifest.write_text(json.dumps(package, indent=2) + "\n", encoding="utf-8")
        run(binary, "test", cwd=project, env=env)
        run(binary, "build", cwd=project, env=env)
        run(binary, "check", "--manifest", "dist/parlando/game.yaml", cwd=project, env=env)
        installed = project / "client/node_modules" / CLIENT / "package.json"
        if json.loads(installed.read_text(encoding="utf-8"))["version"] != version:
            raise RuntimeError(f"{game} did not install the candidate browser client")


def wait_for_host(host, origin):
    """Wait for the candidate's loopback health endpoint with a bounded startup wait."""
    http = build_opener(ProxyHandler({}))
    deadline = time.monotonic() + 20
    while host.poll() is None and time.monotonic() < deadline:
        try:
            with http.open(f"{origin}/health", timeout=1) as response:
                if json.load(response) == {"status": "ok"}:
                    return
        except (URLError, TimeoutError):
            time.sleep(0.1)
    raise RuntimeError("Candidate host failed to become healthy")


@contextmanager
def local_host(work, binary, env):
    """Start the installed host with a temporary store and distinct throwaway credentials."""
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        address = f"127.0.0.1:{listener.getsockname()[1]}"
    origin = f"http://{address}"
    grants = {game: secrets.token_urlsafe(32) for game in GAMES.values()}
    grant_file = work / "grants.json"
    grant_file.write_text(json.dumps(grants), encoding="utf-8")
    credentials = {
        **env, "PARLANDO_HOST_ADMIN_TOKEN": secrets.token_urlsafe(32),
        "PARLANDO_HOST_ADMIN_ORIGIN": origin, "PARLANDO_DEPLOY_ORIGIN": origin,
        "NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1",
    }
    log = work / "host.log"
    with log.open("w", encoding="utf-8") as output, subprocess.Popen(
        [binary, "host", "--bind", address, "--public-url", origin,
         "--store", work / "host", "--grants", grant_file],
        cwd=work, env=credentials, stdout=output, stderr=subprocess.STDOUT,
    ) as host:
        try:
            wait_for_host(host, origin)
            yield origin, credentials, grants
        except BaseException:
            print(log.read_text(encoding="utf-8"), file=sys.stderr)
            raise
        finally:
            if host.poll() is None:
                host.terminate()
                try:
                    host.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    host.kill()


def play_games(work, binary, env):
    """Pin both starters in Typed mode and run the two-browser communication and gameplay gate."""
    browser = work / "browser"
    browser.mkdir()
    typed_config = work / "typed-experiment.yaml"
    typed_config.write_text("communication:\n  mode: typed\n", encoding="utf-8")
    fixtures = ROOT / "client-server-tests/browser"
    playwright = json.loads((fixtures / "package.json").read_text())["devDependencies"]["@playwright/test"]
    for source in (fixtures / "ensure-browser.mjs", ROOT / "scripts/release_candidate_browser.mjs"):
        shutil.copyfile(source, browser / source.name)
    run("npm", "install", "--no-audit", "--no-fund", f"@playwright/test@{playwright}", cwd=browser, env=env)
    run("node", "ensure-browser.mjs", cwd=browser, env=env)
    with local_host(work, binary, env) as (origin, credentials, grants):
        for game in GAMES.values():
            deploy_env = {**credentials, "PARLANDO_DEPLOY_TOKEN": grants[game]}
            receipt = json.loads(run(
                binary, "deploy", "--server", origin, "--package", "dist/parlando", "--json",
                cwd=work / game, env=deploy_env, capture=True, timeout=60,
            ))["result"]
            pin = json.loads(run(
                binary, "select", "--server", origin, "--experiment", game, "--game", game,
                "--revision", receipt["revision"], "--config", typed_config, "--json",
                cwd=work, env=credentials, capture=True, timeout=60,
            ))["result"]
            if pin["url"] != f"{origin}/e/{game}/" or pin["revision"] != receipt["revision"]:
                raise RuntimeError(f"Unexpected experiment pin for {game}: {pin}")
            run(
                binary, "intake", "--server", origin, "--experiment", game, "testing",
                cwd=work, env=credentials, timeout=60,
            )
            run("node", "release_candidate_browser.mjs", pin["url"], cwd=browser, env=env, timeout=60)
            print(f"Completed {game} with two-way Typed chat in Chromium", flush=True)


def main():
    """Run the package-consumer rehearsal in a temporary workspace."""
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--output", type=Path, help="retain the artifacts from a passing run")
    mode.add_argument("--verified", type=Path, help="print the directory of a verified bundle")
    args = parser.parse_args()
    version = package_metadata(ROOT / "rust-server/Cargo.toml")["version"]
    if args.verified:
        print(verified_bundle(args.verified.resolve(), version, os.environ))
        return
    with tempfile.TemporaryDirectory(prefix="parlando-release-") as directory:
        work = Path(directory).resolve()
        artifacts = work / "artifacts"
        artifacts.mkdir()
        env = {**os.environ, "CARGO_TARGET_DIR": str(work / "cargo-target")}
        env.pop("PYTHONPATH", None)
        env.pop("NODE_PATH", None)
        env.setdefault("PLAYWRIGHT_BROWSERS_PATH", str(work / "chromium"))
        sources = release_sources(env) if args.output else None
        stage_packages(work, env)
        if args.output:
            check_sources(sources, env)
        binary, packaged_server = install_rust(work, artifacts, version, env)
        client = pack_client(work, artifacts, env)
        venv = install_python(work, artifacts, packaged_server, version, env)
        env["VIRTUAL_ENV"] = str(venv)
        build_games(work, binary, client, version, env)
        play_games(work, binary, env)
        if args.output:
            save_verified(args.output.resolve(), artifacts, version, sources)
    print(f"Parlando {version} release candidate passed")


if __name__ == "__main__":
    main()
