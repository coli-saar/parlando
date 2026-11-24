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
    ("parlando-game-sdk", "parlando_game_sdk", "parlando-python-sdk"),
    ("parlando-remote-agent", "parlando_agent_sdk", "parlando-agent-sdk"),
)
GAMES = {"rust": "release-rust", "python": "release-python"}
TICK_AGENT_ID = "beacon.tick_on_request"
AGENT_ID = "release.beacon"
AGENT_NAME = "Release Beacon partner"
AGENT_MESSAGE = "One tick from the Beacon partner."
AGENT_FIXTURE = ROOT / "scripts/release_candidate_agent.rs"


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


def install_python(work, packages, packaged_server, version, env):
    """Install the wheels built by Make and exercise the installed SDKs."""
    wheels = [packages / f"{name.replace('-', '_')}-{version}-py3-none-any.whl" for name, _, _ in PYTHON_SDKS]
    venv = work / "python"
    run(sys.executable, "-m", "venv", venv, cwd=work, env=env)
    python = venv / "bin/python"
    run(
        python, "-m", "pip", "install", *wheels, "componentize-py==0.25.1",
        cwd=work, env=env,
    )
    imports = "\n".join(
        f"import {module}\nassert metadata.version({name!r}) == {version!r}"
        for name, module, _ in PYTHON_SDKS
    )
    run(python, "-I", "-c", f"from importlib import metadata\n{imports}", cwd=work, env=env)
    run(python, "-m", "pip", "check", cwd=work, env=env)

    # The SDK tests run without their source trees. The agent protocol check uses the packaged proto.
    shutil.copytree(packaged_server / "proto", work / "rust-server/proto")
    for _, _, directory in PYTHON_SDKS:
        tests = work / directory / "tests"
        shutil.copytree(ROOT / directory / "tests", tests, ignore=shutil.ignore_patterns("__pycache__"))
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
    game = work / GAMES["rust"] / "dist" / f"{GAMES['rust']}-0.1.0.zip"
    run(binary, "agent", "set-game", game, cwd=project, env=env)
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
    return run_agent_dependency_experiments(work, binary, archive, env)


def run_agent_dependency_experiments(work, binary, partner, env):
    """Use generated bindings with pinned ZIPs and explicitly switch game builds."""
    project = work / "dependency-policy"
    agent_id = TICK_AGENT_ID
    game = work / GAMES["rust"]
    archive = work / "pinned-beacon.zip"
    shutil.copyfile(game / "dist" / f"{GAMES['rust']}-0.1.0.zip", archive)
    created = json.loads(run(
        binary, "agent", "new", agent_id, "--game", archive, "--path", project, "--json",
        cwd=work, env=env, capture=True,
    ))["result"]
    manifest = project / "agent.yaml"
    pinned = created["dependency"]
    if pinned["sha256"] != hashlib.sha256(archive.read_bytes()).hexdigest():
        raise RuntimeError("Creating the agent did not pin its game ZIP's SHA-256")

    hidden = game.with_name("offline-beacon")
    game.rename(hidden)
    try:
        run(binary, "check", "--manifest", manifest, cwd=project, env=env)
        run(binary, "build", "--manifest", manifest, cwd=project, env=env)
        run_dependency_experiment(binary, project, manifest, partner, agent_id, "archive", 4, env)
    finally:
        hidden.rename(game)

    rules = game / "src/lib.rs"
    original = rules.read_text(encoding="utf-8")
    target = "target: 4 + (seed % 3) as u8,"
    if original.count(target) != 1:
        raise RuntimeError("Unexpected Beacon target initialization")
    bindings = project / "src/game.rs"
    released_bindings = bindings.read_bytes()
    draft = game / "rehearsal-draft.yaml"
    try:
        updated = build_beacon_draft(
            binary, game, draft, "0.1.1",
            original.replace(target, "target: 7 + (seed % 3) as u8,"), env,
        )
        run(binary, "agent", "set-game", updated, cwd=project, env=env)
        if hashlib.sha256(updated.read_bytes()).hexdigest().encode() not in manifest.read_bytes():
            raise RuntimeError("Switching game builds did not update the recorded SHA-256")
        run_dependency_experiment(binary, project, manifest, partner, agent_id, "updated", 7, env)
        incompatible = build_beacon_draft(
            binary, game, draft, "0.1.2", original.replace("ticks", "strength"), env,
        )
        run(binary, "agent", "set-game", incompatible, cwd=project, env=env)
        if b"pub strength:" not in bindings.read_bytes():
            raise RuntimeError("Selecting a ZIP with a new schema did not refresh the action binding")
        obsolete = subprocess.run(
            [binary, "check", "--manifest", manifest], cwd=project, env=env,
            text=True, capture_output=True,
        )
        if obsolete.returncode != 1 or "has no field named `ticks`" not in obsolete.stderr:
            raise RuntimeError(f"Expected the old action field to fail compilation: {obsolete.stderr}")
        restored = json.loads(run(
            binary, "agent", "set-game", archive, "--json",
            cwd=project, env=env, capture=True,
        ))["result"]
        if bindings.read_bytes() != released_bindings:
            raise RuntimeError("Selecting the original ZIP did not restore its bindings")
        run_dependency_experiment(binary, project, manifest, partner, agent_id, "restored", 4, env)
        if restored["dependency"] != pinned:
            raise RuntimeError("Selecting the original ZIP did not restore its game dependency")
        built = json.loads(run(binary, "build", "--json", cwd=project, env=env, capture=True))
        return Path(built["result"]["package"])
    finally:
        rules.write_text(original, encoding="utf-8")
        draft.unlink(missing_ok=True)


def build_beacon_draft(binary, game, manifest, version, rules, env):
    """Build a headless Beacon ZIP from modified rules with a distinct game version."""
    (game / "src/lib.rs").write_text(rules, encoding="utf-8")
    manifest.write_text(json.dumps({
        "id": GAMES["rust"], "name": "Beacon Relay", "version": version,
        "cargo_manifest": "Cargo.toml",
    }) + "\n", encoding="utf-8")
    build = json.loads(run(
        binary, "build", "--manifest", manifest, "--json",
        cwd=game, env=env, capture=True,
    ))["result"]
    return Path(build["package"])


def run_dependency_experiment(binary, project, manifest, partner, agent_id, mode, target, env):
    """Check that the project-selected game completes with its policy and a sibling ZIP."""
    output = project / f"results-{mode}"
    spec = project / f"experiment-{mode}.yaml"
    spec.write_text(json.dumps({
        "schema": "parlando-agent-experiment/v1",
        "game": {"id": GAMES["rust"]},
        "agents": {"generated": {"factory": agent_id}, "partner": {"factory": AGENT_ID}},
        "sessions": {
            "scenarios": [{"name": "seeded-beacon", "config": {}, "seeds": [0]}],
            "seats": {"player_a": "generated", "player_b": "partner"},
        },
        "schedule": {"kind": "alternate_every_response", "first": "player_a"},
        "output": {"directory": str(output), "trace": "full"},
    }) + "\n", encoding="utf-8")
    summary = json.loads(run(
        binary, "run", spec, "--manifest", manifest, "--agent", partner, "--json",
        cwd=project, env=env, capture=True,
    ))["result"]
    if (summary["completed"], summary["failed"]) != (1, 0):
        raise RuntimeError(f"Unexpected dependency run summary for {mode}: {summary}")
    results = list((output / "results").glob("*.json"))
    if len(results) != 1:
        raise RuntimeError(f"Unexpected result files for {mode}: {results}")
    result = json.loads(results[0].read_text(encoding="utf-8"))
    trace = result["trace"]
    opening = trace[0]["response"]
    if trace[0]["role"] != "A" or opening["type"] != "message" or "charge the beacon" not in opening["message"]:
        raise RuntimeError(f"Generated Beacon partner did not open with tick instructions: {opening}")
    requests = 0
    for turn in trace[1:]:
        response = turn["response"]
        if turn["role"] == "B" and response.get("message") == "charge the beacon":
            requests += 1
        if turn["role"] == "A" and response["type"] != "yield":
            if requests == 0 or response != {"type": "action", "action": {"ticks": 1}}:
                raise RuntimeError(f"Unexpected generated policy decision: {turn}")
            requests -= 1
    seats = {(agent["role"], agent["factory"]) for agent in result["agents"]}
    if (result["completion"], seats) != (
        {"activated_at": target}, {("A", agent_id), ("B", AGENT_ID)},
    ):
        raise RuntimeError(f"Unexpected Beacon result with {mode}: {result}")


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
        **env,
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
            if not url.startswith(f"{admin_origin}/?cli-login#"):
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
    """Deploy both starters, then pilot them through the game dashboards."""
    browser = work / "browser"
    browser.mkdir()
    fixtures = ROOT / "client-server-tests/browser"
    playwright = json.loads((fixtures / "package.json").read_text())["devDependencies"]["@playwright/test"]
    for source in (fixtures / "ensure-browser.mjs", fixtures / "browser-driver.mjs", ROOT / "scripts/release_candidate_browser.mjs"):
        shutil.copyfile(source, browser / source.name)
    run("npm", "install", "--no-audit", "--no-fund", f"@playwright/test@{playwright}", cwd=browser, env=env)
    run("node", "ensure-browser.mjs", cwd=browser, env=env)
    browser_env = {**env, "PARLANDO_REHEARSAL_PASSWORD": secrets.token_urlsafe(32)}
    # Exercise the shipped JS, game and agent together with a local transcription provider.
    game = GAMES["rust"]
    extracted = work / "speech-startup"
    with ZipFile(work / game / "dist" / f"{game}-0.1.0.zip") as archive:
        archive.extractall(extracted)
    run("cargo", "test", "--locked", "--test", "browser_e2e", "speech_agent_startup", "--", "--ignored",
        cwd=ROOT / "client-server-tests", env={
            **env, "PARLANDO_BROWSER_DRIVER_DIR": str(browser),
            "PARLANDO_STARTUP_PACKAGE_DIR": str(extracted), "PARLANDO_STARTUP_AGENT": str(agent_package),
        })
    with local_host(work, binary, env) as (origin, admin_origin, credentials):
        run("node", "release_candidate_browser.mjs", "setup", f"{admin_origin}/",
            cwd=browser, env=browser_env, timeout=60)
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
            run(
                "node", "release_candidate_browser.mjs", admin_origin, origin, game,
                cwd=browser, env=browser_env, timeout=60,
            )
            if game == GAMES["rust"]:
                run(
                    "node", "release_candidate_browser.mjs", admin_origin, origin, game, TICK_AGENT_ID,
                    cwd=browser, env=browser_env, timeout=60,
                )
                print("Completed hosted Beacon Relay against the standalone Rust agent", flush=True)
            print(f"Completed {game} with two-way Typed chat in Chromium", flush=True)


def main():
    """Run the package-consumer rehearsal in a temporary workspace."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--packages", required=True, type=Path, help="directory containing the npm tarball and SDK wheels built by Make")
    packages = parser.parse_args().packages.resolve()
    version = package_metadata(ROOT / "rust-server/Cargo.toml")["version"]
    with tempfile.TemporaryDirectory(prefix="parlando-release-") as directory:
        work = Path(directory).resolve()
        artifacts = work / "artifacts"
        artifacts.mkdir()
        env = {**os.environ, "CARGO_TARGET_DIR": str(work / "cargo-target")}
        env.pop("PYTHONPATH", None)
        env.pop("NODE_PATH", None)
        env.setdefault("PLAYWRIGHT_BROWSERS_PATH", str(work / "chromium"))
        for _, source in CRATES:
            stage_crate(work, source, env)
        binary, packaged_server = install_rust(work, artifacts, version, env)
        client = packages / f"coli-saar-parlando-client-{version}.tgz"
        venv = install_python(work, packages, packaged_server, version, env)
        env["VIRTUAL_ENV"] = str(venv)
        build_games(work, binary, client, version, env)
        agent_package = build_and_run_agent(work, binary, version, env)
        play_games(work, binary, agent_package, env)
    print(f"Parlando {version} release candidate passed")


if __name__ == "__main__":
    main()
