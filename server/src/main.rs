use std::io::IsTerminal;
use std::process::ExitCode;

use fuwa_server::cluster::Role;
use fuwa_server::config::Config;

const HELP: &str = "\
fuwa: a self-hostable, Discord-like chat server

Usage: fuwa [serve]           run the server
       fuwa health            exit 0 if the server on FUWA_PORT answers (for health checks)
       fuwa to-sqlite FILE…   switch database files back to plain SQLite so other
                              SQLite tools can open them (stop fuwa first; it
                              switches them back to concurrent writes on start)
       fuwa restore [DIR]     restore this part of a split instance (FUWA_ROLE)
                              from its replica (FUWA_S3_* or FUWA_REPLICA_PATH)
                              into DIR, default FUWA_DATA_PATH: a directory's
                              node.db and pictures, or a shard's servers
       fuwa restore --server ID [DIR]
                              restore one community server into DIR/servers
                              (a server moving to another shard)
       fuwa restore --list    show what the replica holds

Everything is configured with FUWA_* environment variables (or a .env file);
see https://github.com/waifu-devs/fuwa#configuration. The most common:

  FUWA_DATA_PATH        where the databases live (default ~/.fuwa)
  FUWA_PORT             port to listen on (default 8080)
  FUWA_PUBLIC_URL       the URL clients reach this instance on
  FUWA_LOCAL_ACCOUNTS   open | closed | off (default open)
  FUWA_TELEMETRY        on | off: the anonymous daily usage signal (default on)
  FUWA_ROLE             all | gateway | directory | shard | media: run one part
                        of a split instance (default all, everything in one process)
  FUWA_MEDIA_PORT       the port calls' sound uses, UDP and TCP (default 50000;
                        off for no calls in this process)
  FUWA_MEDIA_ADDRESSES  where apps reach it (default this machine's address)
  FUWA_S3_BUCKET        on a split instance's directory and shards: a bucket
                        to copy their databases and pictures to as they change
                        (with FUWA_S3_ENDPOINT, FUWA_S3_ACCESS_KEY_ID and
                        FUWA_S3_SECRET_ACCESS_KEY); FUWA_REPLICA_PATH for a folder
  FUWA_RESTORE          if-empty: restore from the replica when the data
                        directory is empty (default off)
";

#[tokio::main]
async fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => {}
        Some("-V" | "--version" | "version") => {
            println!("fuwa {}", fuwa_server::VERSION);
            return ExitCode::SUCCESS;
        }
        Some("health") => return health().await,
        Some("to-sqlite") => return to_sqlite(std::env::args().skip(2).collect()).await,
        Some("restore") => return restore(std::env::args().skip(2).collect()).await,
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("fuwa: unknown command {other:?}\n\n{HELP}");
            return ExitCode::from(2);
        }
    }

    let config = match Config::load() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("fuwa: {err}");
            return ExitCode::from(2);
        }
    };

    let filter = std::env::var("FUWA_LOG").unwrap_or_else(|_| "info,turso_core=warn".into());
    // Colors only on a terminal, so container logs stay plain text.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_ansi(std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
        .init();

    match fuwa_server::app::run(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("{err}");
            ExitCode::FAILURE
        }
    }
}

/// Asks the local server's /healthz whether it's up.
async fn health() -> ExitCode {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let port = Config::load().map(|config| config.port).unwrap_or(8080);
    let check = async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        stream.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await?;
        Ok::<_, std::io::Error>(response.starts_with("HTTP/1.1 200"))
    };
    match tokio::time::timeout(std::time::Duration::from_secs(5), check).await {
        Ok(Ok(true)) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

/// Switches database files from Turso's concurrent-writer mode back to plain
/// SQLite (WAL), for tools that don't read the former.
async fn to_sqlite(files: Vec<String>) -> ExitCode {
    if files.is_empty() {
        eprintln!("fuwa: name the database files, e.g. fuwa to-sqlite ~/.fuwa/servers/*.db");
        return ExitCode::from(2);
    }
    let key = match Config::load() {
        Ok(config) => config.encryption_key,
        Err(err) => {
            eprintln!("fuwa: {err}");
            return ExitCode::from(2);
        }
    };
    let mut failed = false;
    for file in &files {
        match fuwa_server::db::to_sqlite(std::path::Path::new(file), key.as_ref()).await {
            Ok(()) => println!("{file}: plain SQLite{}", if key.is_some() { " (still encrypted)" } else { "" }),
            Err(err) => {
                eprintln!("{file}: {err}");
                failed = true;
            }
        }
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// Restores this part of a split instance from its replica into an empty
/// directory, or lists what the replica holds.
async fn restore(args: Vec<String>) -> ExitCode {
    let config = match Config::load() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("fuwa: {err}");
            return ExitCode::from(2);
        }
    };
    let Some(replica) = &config.replica else {
        eprintln!(
            "fuwa: no replica to restore from; set FUWA_S3_BUCKET (and the other FUWA_S3_*) or FUWA_REPLICA_PATH"
        );
        return ExitCode::from(2);
    };
    let store = match replica.store() {
        Ok(store) => store,
        Err(err) => {
            eprintln!("fuwa: {err}");
            return ExitCode::from(2);
        }
    };
    if args.first().map(String::as_str) == Some("--list") {
        return match fuwa_server::replica::describe(&store).await {
            Ok(lines) => {
                println!("{}", store.describe());
                for line in lines {
                    println!("  {line}");
                }
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("fuwa: {err}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().map(String::as_str) == Some("--server") {
        let Some(id) = args.get(1) else {
            eprintln!("fuwa: restore --server needs a server id");
            return ExitCode::from(2);
        };
        let dir = args.get(2).map(std::path::PathBuf::from).unwrap_or_else(|| config.data_path.clone());
        let dest = dir.join("servers").join(format!("{id}.db"));
        if fuwa_server::id::parse_id("server", id).ok().as_deref() != Some(id.as_str()) {
            eprintln!("fuwa: {id:?} isn't a server id");
            return ExitCode::from(2);
        }
        if dest.exists() {
            eprintln!("fuwa: {} already exists", dest.display());
            return ExitCode::from(2);
        }
        if let Err(err) = std::fs::create_dir_all(dir.join("servers")) {
            eprintln!("fuwa: {}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
        let name = fuwa_server::servers::replica_name(id);
        return match fuwa_server::replica::restore_file(&store, &name, &dest, config.encryption_key.as_ref()).await {
            Ok(whole) => {
                println!("restored {} from {}", dest.display(), store.describe());
                if !whole {
                    println!("  its replica stopped short; restored as far as it went");
                }
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("fuwa: {err}");
                ExitCode::FAILURE
            }
        };
    }
    let dir = args.first().map(std::path::PathBuf::from).unwrap_or_else(|| config.data_path.clone());
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("fuwa: {}: {err}", dir.display());
        return ExitCode::FAILURE;
    }
    let key = config.encryption_key.as_ref();
    let restored = match config.cluster.role {
        Role::Shard => {
            // Its name says which servers are its; a made-up one would find none.
            let named = std::fs::read_to_string(dir.join("shard-id")).ok().map(|id| id.trim().to_string());
            let Some(shard) = config.cluster.shard_id.clone().or(named) else {
                eprintln!("fuwa: set FUWA_SHARD_ID to the name of the shard to restore");
                return ExitCode::from(2);
            };
            fuwa_server::replica::restore_shard(&store, &dir, key, &shard).await.map(|restored| (restored, Some(shard)))
        }
        _ => {
            if dir.join("node.db").exists() {
                eprintln!("fuwa: {} already has a node.db; restore into an empty directory", dir.display());
                return ExitCode::from(2);
            }
            fuwa_server::replica::restore_directory(&store, &dir, key).await.map(|restored| (restored, None))
        }
    };
    match restored {
        Ok((restored, None)) if !restored.node => {
            eprintln!("fuwa: {} holds no instance", store.describe());
            ExitCode::FAILURE
        }
        Ok((restored, shard)) => {
            if let Some(shard) = shard {
                println!(
                    "restored {} from {}: {} servers of shard {shard}",
                    dir.display(),
                    store.describe(),
                    restored.servers
                );
            } else {
                println!(
                    "restored {} from {}: node.db{} and {} pictures",
                    dir.display(),
                    store.describe(),
                    if restored.dms { ", dms.db" } else { "" },
                    restored.media
                );
            }
            for name in &restored.incomplete {
                println!("  {name}: its replica stopped short; restored as far as it went");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("fuwa: {err}");
            ExitCode::FAILURE
        }
    }
}
