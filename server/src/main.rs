use std::io::IsTerminal;
use std::process::ExitCode;

use fuwa_server::config::Config;

const HELP: &str = "\
fuwa: a self-hostable, Discord-like chat server

Usage: fuwa [serve]    run the server
       fuwa health     exit 0 if the server on FUWA_PORT answers (for health checks)

Everything is configured with FUWA_* environment variables (or a .env file);
see https://github.com/waifu-devs/fuwa#configuration. The most common:

  FUWA_DATA_PATH        where the databases live (default ~/.fuwa)
  FUWA_PORT             port to listen on (default 8080)
  FUWA_PUBLIC_URL       the URL clients reach this instance on
  FUWA_LOCAL_ACCOUNTS   open | closed | off (default open)
  FUWA_TELEMETRY        on | off: the anonymous daily usage signal (default on)
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
