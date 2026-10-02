use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protos = ["types", "node", "auth", "account", "server", "channel", "message", "event", "admin", "media"]
        .map(|name| PathBuf::from(format!("../proto/fuwa/v1/{name}.proto")));
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    let mut config = tonic_prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);

    let includes = [PathBuf::from("../proto"), protoc_bin_vendored::include_path()?];
    tonic_prost_build::configure()
        .build_client(true)
        .file_descriptor_set_path(out_dir.join("fuwa_descriptor.bin"))
        .compile_with_config(config, &protos, &includes)?;

    // How the parts of a split instance talk to each other. Kept out of the
    // descriptor set above, so reflection shows clients only their API, and
    // written to a folder of its own, so it doesn't replace fuwa.v1's code.
    let cluster = out_dir.join("cluster");
    std::fs::create_dir_all(&cluster)?;
    let mut config = tonic_prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
    tonic_prost_build::configure().out_dir(cluster).extern_path(".fuwa.v1", "crate::pb").compile_with_config(
        config,
        &[PathBuf::from("../proto/fuwa/cluster/v1/cluster.proto")],
        &includes,
    )?;

    println!("cargo:rerun-if-changed=../proto");
    Ok(())
}
