use std::path::PathBuf;

/// The client side of the protocol (proto/fuwa/v1), the same files the server builds from.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protos = [
        "types", "node", "auth", "account", "server", "channel", "message", "event", "admin", "media", "role",
        "invite", "join", "dm", "emoji", "webhook", "agent", "automod", "call", "sso",
    ]
    .map(|name| PathBuf::from(format!("../proto/fuwa/v1/{name}.proto")));
    let mut config = tonic_prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
    let includes = [PathBuf::from("../proto"), protoc_bin_vendored::include_path()?];
    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .build_transport(false)
        .compile_with_config(config, &protos, &includes)?;
    println!("cargo:rerun-if-changed=../proto");
    Ok(())
}
