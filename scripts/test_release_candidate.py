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
from zipfile import ZipFile


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
AGENT_ID = "release.beacon"
AGENT_NAME = "Release Beacon partner"
AGENT_MESSAGE = "One tick from the Beacon partner."
AGENT_FIXTURE = ROOT / "scripts/release_candidate_agent.rs"
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
    files = {
        ROOT / ".cargo/config.toml",
        Path(__file__).resolve(),
        AGENT_FIXTURE,
        ROOT / "scripts/release_candidate_browser.mjs",
    }
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
        built = json.loads(run(binary, "build", "--json", cwd=project, env=env, capture=True))
        archive = Path(built["result"]["package"])
        if archive != project.resolve() / "dist" / f"{game}-0.1.0.zip" or not archive.is_file():
            raise RuntimeError(f"Unexpected starter archive: {archive}")
        run(binary, "check", "--package", archive, cwd=project, env=env)
        installed = project / "client/node_modules" / CLIENT / "package.json"
        if json.loads(installed.read_text(encoding="utf-8"))["version"] != version:
            raise RuntimeError(f"{game} did not install the candidate browser client")


def build_and_run_agent(work, binary, version, env):
    """Compile a standalone Rust policy with the extracted SDK and run both seats."""
    project = work / "beacon-policy"
    (project / "src").mkdir(parents=True)
    shutil.copyfile(AGENT_FIXTURE, project / "src/lib.rs")
    (project / "Cargo.toml").write_text(
        "[package]\n"
        'name = "release-beacon-policy"\n'
        'version = "0.1.0"\n'
        'edition = "2021"\n'
        "\n[lib]\n"
        'crate-type = ["cdylib"]\n'
        "\n[dependencies]\n"
        f'parlando-game-sdk = "={version}"\n'
        'serde = { version = "1", features = ["derive"] }\n',
        encoding="utf-8",
    )
    metadata = json.loads(run(
        "cargo", "metadata", "--format-version", "1", cwd=project, env=env, capture=True,
    ))
    sdk_manifest = work / "crates" / f"parlando-game-sdk-{version}" / "Cargo.toml"
    resolved_sdk = [
        Path(package["manifest_path"]).resolve() for package in metadata["packages"]
        if package["name"] == "parlando-game-sdk"
    ]
    if resolved_sdk != [sdk_manifest.resolve()]:
        raise RuntimeError(f"Standalone policy resolved a different game SDK: {resolved_sdk}")
    run("cargo", "check", "--locked", cwd=project, env=env)
    manifest = project / "agent.yaml"
    manifest.write_text(json.dumps({
        "game": GAMES["rust"], "id": AGENT_ID, "name": AGENT_NAME,
        "version": "0.1.0", "cargo_manifest": "Cargo.toml",
    }) + "\n", encoding="utf-8")
    built = json.loads(run(
        binary, "build", "--manifest", manifest, "--json",
        cwd=project, env=env, capture=True,
    ))["result"]
    archive = Path(built["package"])
    if archive != project / "dist" / f"agent-{AGENT_ID}-0.1.0.zip":
        raise RuntimeError(f"Unexpected standalone policy archive: {archive}")
    with ZipFile(archive) as package:
        if set(package.namelist()) != {"agent.yaml", "agent.wasm"}:
            raise RuntimeError("Policy archive must contain an agent manifest and its own component")
        policy_digest = hashlib.sha256(package.read("agent.wasm")).hexdigest()
    checked = json.loads(run(
        binary, "check", "--package", archive, "--json", cwd=project, env=env, capture=True,
    ))["result"]
    if (
        checked["kind"], checked["game"], checked["id"], checked["agent_version"],
        checked["component_sha256"], built["component_sha256"],
    ) != ("agent", GAMES["rust"], AGENT_ID, "1", policy_digest, policy_digest):
        raise RuntimeError(f"Unexpected standalone policy inspection: {checked}")

    run_agent_experiments(work, project, binary, archive, policy_digest, env)
    return archive


def run_agent_experiments(work, project, binary, archive, policy_digest, env):
    """Check both shared metering modes with the same three deterministic seeds."""
    seeds = [0, 1, 2]
    game = work / GAMES["rust"] / "dist" / f"{GAMES['rust']}-0.1.0.zip"
    for mode in ("fuel", "none"):
        output = project / f"results-{mode}"
        spec = project / f"experiment-{mode}.yaml"
        spec.write_text(json.dumps({
            "schema": "parlando-agent-experiment/v1",
            "game": {"id": GAMES["rust"]},
            "agents": {"policy": {"factory": AGENT_ID}},
            "sessions": {
                "scenarios": [{"name": "seeded-beacon", "config": {}, "seeds": seeds}],
                "seats": {"player_a": "policy", "player_b": "policy"},
            },
            "schedule": {"kind": "alternate_every_response", "first": "player_a"},
            "execution": {"wasm_metering": mode},
            "output": {"directory": str(output), "trace": "full"},
        }) + "\n", encoding="utf-8")
        summary = json.loads(run(
            binary, "run", spec, "--package", game, "--agent", archive, "--json",
            cwd=project, env=env, capture=True, timeout=60,
        ))["result"]
        if (summary["completed"], summary["failed"]) != (len(seeds), 0):
            raise RuntimeError(f"Unexpected standalone policy run summary for {mode}: {summary}")
        results = [
            json.loads(path.read_text(encoding="utf-8"))
            for path in (output / "results").glob("*.json")
        ]
        if sorted(result["game_seed"] for result in results) != seeds:
            raise RuntimeError(f"Standalone policy run did not record the three seeds for {mode}")
        for result in results:
            check_agent_result(result, policy_digest)
        print(f"Completed three headless Beacon sessions with WASM metering {mode}", flush=True)


def check_agent_result(result, policy_digest):
    """Verify the exact Beacon trace and packaged policy provenance in one session."""
    target = 4 + result["game_seed"] % 3
    if (
        result["status"], result["completion"],
        result["accepted_actions"], result["rejected_actions"], result["messages"],
    ) != ("completed", {"activated_at": target}, target, 0, 1):
        raise RuntimeError(f"Unexpected Beacon outcome or counters: {result}")
    agents = {
        (agent["role"], agent["factory"], agent["name"], agent["version"], agent["wasm_sha256"])
        for agent in result["agents"]
    }
    if agents != {
        (role, AGENT_ID, AGENT_NAME, "1", policy_digest) for role in ("A", "B")
    }:
        raise RuntimeError(f"Unexpected Beacon policy provenance: {agents}")
    trace = result["trace"]
    if len(trace) != target:
        raise RuntimeError(f"Beacon should need exactly {target} decisions: {trace}")
    for charge, row in enumerate(trace):
        role = "A" if charge % 2 == 0 else "B"
        response = {"type": "action", "action": {"ticks": 1}}
        if charge == 1:
            response.update(type="action_and_message", message=AGENT_MESSAGE)
        if (
            row["role"], row["reason"], row["response"], row["rejection"], row["input"]["observation"],
        ) != (
            role, "scheduled", response, None,
            {"charge": charge, "target": target, "your_turn": True},
        ):
            raise RuntimeError(f"Unexpected Beacon decision at charge {charge}: {row}")


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
        port = listener.getsockname()[1]
    address = f"127.0.0.1:{port}"
    origin = f"http://{address}"
    admin_origin = f"http://localhost:{port}"
    credentials = {
        **env, "PARLANDO_HOST_ADMIN_TOKEN": secrets.token_urlsafe(32),
        "PARLANDO_HOST_ADMIN_ORIGIN": origin,
        "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost",
    }
    credentials.pop("PARLANDO_ADMIN_URL", None)
    log = work / "host.log"
    with log.open("w", encoding="utf-8") as output, subprocess.Popen(
        [binary, "serve", "--bind", address,
         "--store", work / "host"],
        cwd=work, env=credentials, stdout=output, stderr=subprocess.STDOUT,
    ) as host:
        try:
            wait_for_host(host, origin)
            yield origin, admin_origin, credentials
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


def authorize_cli(binary, origin, admin_origin, browser, env):
    """Complete the installed CLI's pairing with the disposable browser account."""
    with subprocess.Popen(
        [binary, "login", "--server", origin, "--no-browser"], stderr=subprocess.PIPE,
        text=True, env=env,
    ) as login:
        try:
            prompt = login.stderr.readline()
            url = prompt.strip().removeprefix("Authorize this CLI in your browser: ")
            if not url.startswith(f"{admin_origin}/admin?cli-login#"):
                raise RuntimeError("CLI did not print the expected authorization link")
            subprocess.run(
                ["node", "release_candidate_browser.mjs", "login"],
                input=url, text=True, check=True, cwd=browser, env=env, timeout=60,
            )
            if login.wait(timeout=20):
                raise RuntimeError(login.stderr.read())
        finally:
            if login.poll() is None:
                login.kill()


def play_games(work, binary, agent_package, env):
    """Deploy and activate both starters, then pilot them through the game dashboards."""
    browser = work / "browser"
    browser.mkdir()
    fixtures = ROOT / "client-server-tests/browser"
    playwright = json.loads((fixtures / "package.json").read_text())["devDependencies"]["@playwright/test"]
    for source in (fixtures / "ensure-browser.mjs", ROOT / "scripts/release_candidate_browser.mjs"):
        shutil.copyfile(source, browser / source.name)
    run("npm", "install", "--no-audit", "--no-fund", f"@playwright/test@{playwright}", cwd=browser, env=env)
    run("node", "ensure-browser.mjs", cwd=browser, env=env)
    browser_env = {**env, "PARLANDO_REHEARSAL_PASSWORD": secrets.token_urlsafe(32)}
    with local_host(work, binary, env) as (origin, admin_origin, credentials):
        authorize_cli(binary, origin, admin_origin, browser, browser_env)
        for game in GAMES.values():
            receipt = json.loads(run(
                binary, "deploy", "--host", origin, "--json",
                cwd=work / game, env=credentials, capture=True, timeout=60,
            ))["result"]
            if receipt["game"] != game or receipt["version"] != "0.1.0":
                raise RuntimeError(f"Unexpected deployment receipt for {game}: {receipt}")
            if game == GAMES["rust"]:
                deployed = json.loads(run(
                    binary, "deploy", "--host", origin, "--package", agent_package, "--json",
                    cwd=work, env=credentials, capture=True, timeout=60,
                ))["result"]
                with ZipFile(agent_package) as archive:
                    digest = hashlib.sha256(archive.read("agent.wasm")).hexdigest()
                if (deployed["game"], deployed["component_sha256"]) != (game, digest):
                    raise RuntimeError(f"Host registered the wrong agent: {deployed}")
            activation = json.loads(run(
                binary, "activate", "--host", origin, "--game", game, "--json",
                cwd=work, env=credentials, capture=True, timeout=60,
            ))["result"]
            if activation["active"] is not True:
                raise RuntimeError(f"Candidate host did not activate {game}: {activation}")
            run(
                "node", "release_candidate_browser.mjs", admin_origin, origin, game,
                cwd=browser, env=browser_env, timeout=60,
            )
            if game == GAMES["rust"]:
                run(
                    "node", "release_candidate_browser.mjs", admin_origin, origin, game, AGENT_ID,
                    cwd=browser, env=browser_env, timeout=60,
                )
                print("Completed hosted Beacon Relay against the standalone Rust agent", flush=True)
            run(
                binary, "deactivate", "--host", origin, "--game", game,
                cwd=work, env=credentials, timeout=60,
            )
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
        agent_package = build_and_run_agent(work, binary, version, env)
        play_games(work, binary, agent_package, env)
        if args.output:
            save_verified(args.output.resolve(), artifacts, version, sources)
    print(f"Parlando {version} release candidate passed")


if __name__ == "__main__":
    main()
