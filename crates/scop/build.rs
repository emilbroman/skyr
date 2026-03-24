fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("failed to find protoc");

    // SAFETY: Cargo build scripts run in a single-threaded process. `set_var`
    // is unsafe because mutating the environment is not thread-safe, but here
    // we are the only thread and the variable is consumed only by the
    // `tonic_build` child process spawned below.
    unsafe { std::env::set_var("PROTOC", protoc) };

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/scop.proto"], &["proto"])
        .expect("failed to compile scop protobuf");
}
