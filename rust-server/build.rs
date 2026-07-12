/// Configures protobuf generation and macOS linker flags needed by integration tests.
fn main() {
    #[cfg(target_os = "macos")]
    {
        // LiveKit's prebuilt WebRTC archive contains Objective-C categories that are
        // referenced dynamically at runtime. The final Parlando binaries/tests must
        // ask the macOS linker to retain those category object files, otherwise
        // Room::connect can abort with an unrecognized selector inside WebRTC.
        println!("cargo:rustc-link-arg-tests=-ObjC");
    }

    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc is available");
    std::env::set_var("PROTOC", protoc);
    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&["proto/parlando_agent_v1.proto"], &["proto"])
        .expect("Parlando agent protobuf compiles");
    println!("cargo:rerun-if-changed=proto/parlando_agent_v1.proto");
    emit_build_metadata();
}

fn emit_build_metadata() {
    println!(
        "cargo:rustc-env=PARLANDO_SERVER_BUILD_TIME={}",
        chrono_like_now()
    );
    if let Some(value) = git_output(&["rev-parse", "HEAD"]) {
        println!("cargo:rustc-env=PARLANDO_SERVER_GIT_SHA={value}");
    }
    if let Some(value) = git_output(&["status", "--porcelain"]) {
        println!(
            "cargo:rustc-env=PARLANDO_SERVER_GIT_DIRTY={}",
            if value.is_empty() { "false" } else { "true" }
        );
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn chrono_like_now() -> String {
    std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
