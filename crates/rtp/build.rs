fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("failed to find protoc");
    // build scripts are single-threaded here and set this only for child processes.
    unsafe { std::env::set_var("PROTOC", protoc) };

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/rtp.proto"], &["proto"])
        .expect("failed to compile rtp protobuf");
}
