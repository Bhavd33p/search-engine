// Generates the gRPC client and server code from proto/search.proto.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A vendored protoc binary means nobody needs protobuf installed to build.
    let mut config = tonic_prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
    tonic_prost_build::configure().compile_with_config(config, &["proto/search.proto"], &["proto"])?;
    Ok(())
}
