use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protos = ["types", "node", "auth", "account", "server", "channel", "message", "event", "admin"]
        .map(|name| PathBuf::from(format!("../proto/fuwa/v1/{name}.proto")));
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    let mut config = tonic_prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);

    tonic_prost_build::configure()
        .build_client(true)
        .file_descriptor_set_path(out_dir.join("fuwa_descriptor.bin"))
        .compile_with_config(config, &protos, &[PathBuf::from("../proto"), protoc_bin_vendored::include_path()?])?;

    println!("cargo:rerun-if-changed=../proto");
    Ok(())
}
