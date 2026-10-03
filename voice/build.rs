use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protos = ["types", "call"].map(|name| PathBuf::from(format!("../proto/fuwa/v1/{name}.proto")));
    let includes = [PathBuf::from("../proto"), protoc_bin_vendored::include_path()?];
    let mut config = tonic_prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
    tonic_prost_build::configure().build_server(false).compile_with_config(config, &protos, &includes)?;
    println!("cargo:rerun-if-changed=../proto/fuwa/v1");
    Ok(())
}
