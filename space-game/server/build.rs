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
}
