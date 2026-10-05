use std::path::PathBuf;

/// The client side of the protocol (proto/fuwa/v1), the same files the server builds from.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protos = [
        "types", "node", "auth", "account", "server", "channel", "message", "event", "admin", "media", "role",
        "invite", "join", "dm", "emoji", "webhook", "agent", "automod", "call", "sso", "presence", "search", "friend",
        "command", "gif",
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
    locales()?;
    Ok(())
}

/// The translations (locales/ at the repo root, shared with the web app), built
/// in: every <language>/<namespace>.json, so a new language is only a new folder.
fn locales() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::fs::canonicalize("../locales")?;
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    for language in std::fs::read_dir(&root)? {
        let language = language?.path();
        if !language.is_dir() {
            continue;
        }
        println!("cargo:rerun-if-changed={}", language.display());
        for file in std::fs::read_dir(&language)? {
            let file = file?.path();
            if file.extension().is_some_and(|e| e == "json") {
                let code = language.file_name().and_then(|n| n.to_str()).ok_or("odd folder name")?.to_string();
                let ns = file.file_stem().and_then(|n| n.to_str()).ok_or("odd file name")?.to_string();
                // Only names shaped like ours go into the generated code.
                assert!(is_tag(&code), "locales/{code} isn't a language tag like en, es or pt-BR");
                assert!(
                    !ns.is_empty() && ns.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                    "locales/{code}/{ns}.json: namespace names are lowercase letters, digits and dashes"
                );
                files.push((code, ns, file));
            }
        }
    }
    files.sort();
    let rows: String = files
        .iter()
        .map(|(code, ns, path)| format!("    ({code:?}, {ns:?}, include_str!({:?})),\n", path.display().to_string()))
        .collect();
    let out = PathBuf::from(std::env::var("OUT_DIR")?).join("locales.rs");
    std::fs::write(out, format!("pub(crate) static FILES: &[(&str, &str, &str)] = &[\n{rows}];\n"))?;
    Ok(())
}

/// The same check as core::i18n::is_tag: "en", "es", "pt-BR", "zh-Hant".
fn is_tag(value: &str) -> bool {
    let mut parts = value.split('-');
    let base = parts.next().unwrap_or_default();
    let base_ok = (2..=3).contains(&base.len()) && base.bytes().all(|b| b.is_ascii_lowercase());
    let rest_ok = match (parts.next(), parts.next()) {
        (None, _) => true,
        (Some(region), None) if region.len() == 2 => region.bytes().all(|b| b.is_ascii_uppercase()),
        (Some(script), None) if script.len() == 4 => {
            let b = script.as_bytes();
            b[0].is_ascii_uppercase() && b[1..].iter().all(|c| c.is_ascii_lowercase())
        }
        _ => false,
    };
    value.len() <= 12 && base_ok && rest_ok
}
