//! Exercises project commands from an isolated CLI installation.

use serde_json::{json, Value};
use std::{
    env, fs,
    path::Path,
    process::{Command, Output},
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const CLIENT: &str = "@coli-saar/parlando-client";

/// Captures both streams so JSON parsing also checks for stray stdout diagnostics.
fn invoke(binary: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(binary)
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run the Parlando CLI")
}

/// Parses the generated package manifest to read its browser SDK dependency.
fn client_dependency(project: &Path) -> String {
    let manifest = fs::read(project.join("client/package.json")).unwrap();
    let package: Value = serde_json::from_slice(&manifest).unwrap();
    package["dependencies"][CLIENT].as_str().unwrap().to_owned()
}

/// New starters honor the matching installed tarball and emit both output formats.
#[test]
fn scaffold_selects_client_from_the_installation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let binary = root.join(format!("parlando{}", env::consts::EXE_SUFFIX));
    fs::copy(env!("CARGO_BIN_EXE_parlando"), &binary).unwrap();

    // Portable CLI archives contain the guest SDK directly under share/.
    let sdk = root.join("share/parlando-python-sdk/src/parlando_game_sdk");
    fs::create_dir_all(&sdk).unwrap();
    fs::write(sdk.join("__init__.py"), "# portable SDK marker\n").unwrap();

    let registry = root.join("registry");
    let human = invoke(&binary, &root, &["new", "registry", "--lang", "python"]);
    assert!(human.status.success(), "{human:?}");
    let output = String::from_utf8(human.stdout).unwrap();
    assert!(output.starts_with("Created game project in "), "{output}");
    let manifest = Path::new("registry").join("game.yaml");
    assert!(
        output.contains(&format!("Manifest: {}", manifest.display())),
        "{output}"
    );
    assert_eq!(client_dependency(&registry), VERSION);
    assert_eq!(
        fs::read_to_string(
            registry.join("vendor/parlando-python-sdk/src/parlando_game_sdk/__init__.py")
        )
        .unwrap(),
        "# portable SDK marker\n"
    );

    let archive = format!("coli-saar-parlando-client-{VERSION}.tgz");
    let assets = root.join("share/parlando").join(VERSION);
    fs::create_dir_all(&assets).unwrap();
    let tarball = b"browser tarball fixture";
    fs::write(assets.join(&archive), tarball).unwrap();

    let bundled = root.join("bundled");
    let structured = invoke(
        &binary,
        &root,
        &[
            "new",
            "bundled",
            "--lang",
            "python",
            "--path",
            bundled.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(structured.status.success(), "{structured:?}");
    let report: Value = serde_json::from_slice(&structured.stdout).unwrap();
    assert_eq!(
        report,
        json!({
            "ok": true,
            "command": "new",
            "result": {"project": bundled, "manifest": bundled.join("game.yaml")}
        })
    );
    assert_eq!(
        client_dependency(&bundled),
        format!("file:../vendor/{archive}")
    );
    assert_eq!(
        fs::read(bundled.join("vendor").join(archive)).unwrap(),
        tarball
    );
    assert!(fs::read_to_string(bundled.join("README.md"))
        .unwrap()
        .contains(&format!("parlando-python-sdk=={VERSION}")));
}

/// Project errors use stderr for humans and a single structured stdout result for automation.
#[test]
fn missing_project_manifest_uses_the_requested_output_format() {
    let temporary = tempfile::tempdir().unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_parlando"));
    for command in ["check", "test", "build"] {
        let human = invoke(binary, temporary.path(), &[command]);
        assert_eq!(human.status.code(), Some(1));
        assert!(human.stdout.is_empty());
        assert!(String::from_utf8(human.stderr)
            .unwrap()
            .contains("game.yaml"));

        let structured = invoke(binary, temporary.path(), &[command, "--json"]);
        assert_eq!(structured.status.code(), Some(1));
        let report: Value = serde_json::from_slice(&structured.stdout).unwrap();
        assert_eq!(report["ok"], false);
        assert_eq!(report["command"], command);
        assert_eq!(report["error"]["code"], "operation_failed");
        assert!(report["error"]["message"]
            .as_str()
            .unwrap()
            .contains("game.yaml"));
    }
}

/// Builds one ZIP and checks it directly while preserving a previous good artifact on failure.
#[test]
fn builds_and_checks_deployment_archive() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_parlando"));
    let component = root.join("fixture.wasm");
    fs::write(
        &component,
        include_bytes!("../src/agent_experiment/tests/headless.wasm"),
    )
    .unwrap();
    fs::create_dir(root.join("client")).unwrap();
    fs::write(root.join("client/index.html"), "<title>Fixture</title>").unwrap();
    let manifest_path = root.join("game.yaml");
    let manifest = "id: zip-fixture\nname: Fixture\nversion: 1.0.0\ncomponent: fixture.wasm\nclient_dist: client\n";
    fs::write(&manifest_path, manifest).unwrap();

    let package = root.join("dist/zip-fixture-1.0.0.zip");
    let human = invoke(binary, &root, &["build"]);
    assert!(human.status.success(), "{human:?}");
    let output = String::from_utf8(human.stdout).unwrap();
    assert_eq!(output.lines().last(), package.to_str());
    assert!(package.is_file());
    assert!(!root.join("dist/parlando").exists());

    let build = invoke(binary, &root, &["build", "--json"]);
    assert!(build.status.success(), "{build:?}");
    let build: Value = serde_json::from_slice(&build.stdout).unwrap();
    assert_eq!(build["result"]["package"], json!(package));
    assert!(build["result"].get("manifest").is_none());

    let check = invoke(
        binary,
        root.parent().unwrap(),
        &["check", "--package", package.to_str().unwrap(), "--json"],
    );
    assert!(check.status.success(), "{check:?}");
    let check: Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(check["result"]["game"], "zip-fixture");
    assert_eq!(
        check["result"]["component_sha256"],
        build["result"]["component_sha256"]
    );

    let previous = fs::read(&package).unwrap();
    fs::write(
        &manifest_path,
        manifest.replace("1.0.0", "1.1.0-rc.1+build.2"),
    )
    .unwrap();
    let versioned = root.join("dist/zip-fixture-1.1.0-rc.1+build.2.zip");
    let build = invoke(binary, &root, &["build"]);
    assert!(build.status.success(), "{build:?}");
    assert!(versioned.is_file());
    assert_eq!(fs::read(&package).unwrap(), previous);

    fs::write(&manifest_path, manifest.replace("1.0.0", "1.2.0")).unwrap();
    let missing = invoke(
        binary,
        &root,
        &["deploy", "--host", "http://127.0.0.1:1", "--json"],
    );
    assert!(!missing.status.success(), "{missing:?}");
    let error: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("zip-fixture-1.2.0.zip"));

    fs::write(&manifest_path, manifest).unwrap();
    fs::write(&component, "invalid wasm").unwrap();
    let failure = invoke(binary, &root, &["build"]);
    assert!(!failure.status.success(), "{failure:?}");
    assert_eq!(fs::read(package).unwrap(), previous);
}

/// An unsafe game ID is rejected before creating an output directory or archive.
#[test]
fn build_rejects_an_unsafe_game_id() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let binary = Path::new(env!("CARGO_BIN_EXE_parlando"));
    fs::write(
        root.join("game.yaml"),
        "id: ../escaped\nname: Fixture\nversion: 1.0.0\ncomponent: fixture.wasm\n",
    )
    .unwrap();
    let build = invoke(binary, root, &["build"]);
    assert!(!build.status.success(), "{build:?}");
    assert!(String::from_utf8(build.stderr)
        .unwrap()
        .contains("IDs must start with"));
    assert!(!root.join("dist").exists());
    assert!(!root.join("escaped-1.0.0.zip").exists());
}
