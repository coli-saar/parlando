/// Configures final binary link flags for native LiveKit/WebRTC support on macOS.
fn main() {
    #[cfg(target_os = "macos")]
    {
        // The LiveKit/WebRTC static archive contains Objective-C categories that
        // are discovered dynamically by Apple's Objective-C runtime. Retaining
        // them in the final binary prevents Room::connect from aborting when
        // WebRTC queries video codec support during peer connection setup.
        println!("cargo:rustc-link-arg-bins=-ObjC");
    }
    emit_build_metadata();
}

fn emit_build_metadata() {
    println!(
        "cargo:rustc-env=PARLANDO_SPACE_GAME_BUILD_TIME={}",
        utc_now()
    );
    if let Some(value) = git_output(&["rev-parse", "HEAD"]) {
        println!("cargo:rustc-env=PARLANDO_SPACE_GAME_GIT_SHA={value}");
    }
    if let Some(value) = git_output(&["status", "--porcelain"]) {
        println!(
            "cargo:rustc-env=PARLANDO_SPACE_GAME_GIT_DIRTY={}",
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

fn utc_now() -> String {
    std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
