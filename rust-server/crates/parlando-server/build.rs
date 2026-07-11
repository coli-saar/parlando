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
}
