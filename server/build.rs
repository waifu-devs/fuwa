use std::{env, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protos = [
        "types", "node", "auth", "account", "server", "channel", "message", "event", "admin", "media", "role",
        "invite", "join", "dm", "automod", "emoji", "webhook", "agent", "call", "sso", "secure",
    ]
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

    // The commit this binary is built from, reported in GetNode. The Docker
    // build has no .git, so the Dockerfile passes FUWA_COMMIT; other builds ask
    // git. It's the same for every build of a commit, so builds stay identical.
    println!("cargo:rerun-if-env-changed=FUWA_COMMIT");
    let commit = match env::var("FUWA_COMMIT") {
        Ok(commit) if !commit.is_empty() => commit,
        _ => git_commit().unwrap_or_default(),
    };
    println!("cargo:rustc-env=FUWA_COMMIT={commit}");
    Ok(())
}

/// HEAD's commit, and a rerun whenever HEAD or the branch it's on moves.
fn git_commit() -> Option<String> {
    let git = |args: &[&str]| {
        let out = Command::new("git").args(args).output().ok().filter(|out| out.status.success())?;
        Some(String::from_utf8(out.stdout).ok()?.trim().to_owned())
    };
    let commit = git(&["rev-parse", "HEAD"])?;
    println!("cargo:rerun-if-changed={}", git(&["rev-parse", "--git-path", "HEAD"])?);
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        // A branch lives in its own file until git packs it into packed-refs.
        let loose = git(&["rev-parse", "--git-path", &branch])?;
        let path =
            if PathBuf::from(&loose).exists() { loose } else { git(&["rev-parse", "--git-path", "packed-refs"])? };
        println!("cargo:rerun-if-changed={path}");
    }
    Some(commit)
}
