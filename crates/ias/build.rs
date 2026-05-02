fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("failed to find protoc");
    // SAFETY: Cargo runs each build script in its own single-threaded process,
    // so there are no concurrent readers of the environment. The variable is set
    // here solely so that `tonic_build` (which spawns `protoc` as a child
    // process) can find the vendored binary.
    unsafe { std::env::set_var("PROTOC", protoc) };

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/ias.proto"], &["proto"])
        .expect("failed to compile ias protobuf");
}
