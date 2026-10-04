//! Where games find Discord: the Unix sockets `discord-ipc-0` to `-9` in
//! the runtime folder (as the libraries search for them), and the Windows
//! pipes `\\.\pipe\discord-ipc-N`. The first free number is taken, so a
//! running Discord keeps 0. Only programs of the same user get in.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Semaphore;

use super::ipc::{self, Command};
use super::{Games, Program};

/// The numbers Discord's libraries try.
const SLOTS: u32 = 10;
/// Games connected at once; more wait.
const MAX_CONNECTIONS: usize = 16;
/// A program says hello this fast or is let go.
const HANDSHAKE_WITHIN: Duration = Duration::from_secs(10);

/// One connection: the handshake, READY, then commands until it goes.
async fn serve<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, games: Arc<Games>, process: Option<String>) {
    let Ok(Ok((op, hello))) = tokio::time::timeout(HANDSHAKE_WITHIN, ipc::read_frame(&mut stream)).await else {
        return;
    };
    let Some(application_id) = (op == ipc::HANDSHAKE).then(|| ipc::handshake(&hello)).flatten() else {
        let _ =
            ipc::write_frame(&mut stream, ipc::CLOSE, &json!({ "code": 4000, "message": "Invalid handshake" })).await;
        return;
    };
    let program = Program::new(process, &application_id);
    let name = program.name.clone();
    let id = games.connect(program);
    // Its activity goes when it does, however the connection ends.
    struct Gone(Arc<Games>, u64);
    impl Drop for Gone {
        fn drop(&mut self) {
            self.0.disconnect(self.1);
        }
    }
    let _gone = Gone(games.clone(), id);
    if ipc::write_frame(&mut stream, ipc::FRAME, &ipc::ready()).await.is_err() {
        return;
    }
    while let Ok((op, value)) = ipc::read_frame(&mut stream).await {
        let reply = match op {
            ipc::FRAME => {
                let (command, answer) = ipc::command(&value, &name, &application_id);
                if let Command::SetActivity(activity) = command {
                    games.set(id, activity.map(|a| *a));
                }
                Some((ipc::FRAME, answer))
            }
            ipc::PING => Some((ipc::PONG, value)),
            ipc::CLOSE => break,
            _ => None,
        };
        if let Some((op, answer)) = reply
            && ipc::write_frame(&mut stream, op, &answer).await.is_err()
        {
            break;
        }
    }
}

/// Listens until the task is stopped. `own` is a folder only this user
/// owns, to tell which user that is.
pub async fn listen(games: Arc<Games>, own: std::path::PathBuf) {
    let limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    imp::listen(games, own, limit).await;
}

#[cfg(unix)]
mod imp {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::Semaphore;

    use super::{Games, SLOTS, serve};

    /// Where the libraries look, in their order.
    fn folder() -> PathBuf {
        ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .find(|p| p.is_dir())
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }

    /// Takes the first number nothing answers on. A socket left behind by
    /// this user is replaced; anyone else's is left alone.
    async fn bind(dir: &Path, uid: u32) -> Option<(UnixListener, PathBuf)> {
        for n in 0..SLOTS {
            let path = dir.join(format!("discord-ipc-{n}"));
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if !meta.file_type().is_socket() || meta.uid() != uid || UnixStream::connect(&path).await.is_ok() {
                    continue;
                }
                let _ = std::fs::remove_file(&path);
            }
            let Ok(listener) = UnixListener::bind(&path) else { continue };
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            return Some((listener, path));
        }
        None
    }

    /// The program on the other end, by its file name.
    fn process(pid: Option<i32>) -> Option<String> {
        let pid = pid.filter(|&p| p > 0)?;
        #[cfg(target_os = "linux")]
        let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        #[cfg(not(target_os = "linux"))]
        let name = {
            let out = std::process::Command::new("ps").args(["-o", "comm=", "-p", &pid.to_string()]).output().ok()?;
            let full = String::from_utf8(out.stdout).ok()?;
            Path::new(full.trim()).file_name()?.to_string_lossy().into_owned()
        };
        let name = name.trim().to_owned();
        (!name.is_empty()).then_some(name)
    }

    pub(super) async fn listen(games: Arc<Games>, own: PathBuf, limit: Arc<Semaphore>) {
        let Ok(uid) = std::fs::metadata(&own).map(|m| m.uid()) else { return };
        let Some((listener, path)) = bind(&folder(), uid).await else {
            tracing::info!("no free place for games to report to");
            return;
        };
        // The socket goes when listening stops.
        struct Remove(PathBuf);
        impl Drop for Remove {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _remove = Remove(path);
        // Connections end with the listener.
        let mut connections = tokio::task::JoinSet::new();
        loop {
            while connections.try_join_next().is_some() {}
            let Ok(permit) = limit.clone().acquire_owned().await else { return };
            let Ok((stream, _)) = listener.accept().await else { continue };
            let Ok(cred) = stream.peer_cred() else { continue };
            if cred.uid() != uid {
                continue;
            }
            let games = games.clone();
            connections.spawn(async move {
                let process = tokio::task::spawn_blocking(move || process(cred.pid())).await.ok().flatten();
                serve(stream, games, process).await;
                drop(permit);
            });
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;
    use std::sync::Arc;

    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use tokio::sync::Semaphore;

    use super::{Games, SLOTS, serve};

    /// A pipe instance only this computer can reach. Pipes get the creator's
    /// default security, which lets other users read but not write, and
    /// the libraries open them for both.
    fn create(name: &str, first: bool) -> std::io::Result<NamedPipeServer> {
        ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).create(name)
    }

    pub(super) async fn listen(games: Arc<Games>, _own: PathBuf, limit: Arc<Semaphore>) {
        // The first number no one else has made a pipe for.
        let Some((name, mut server)) = (0..SLOTS).find_map(|n| {
            let name = format!(r"\\.\pipe\discord-ipc-{n}");
            create(&name, true).ok().map(|s| (name, s))
        }) else {
            tracing::info!("no free place for games to report to");
            return;
        };
        // Connections end with the listener.
        let mut connections = tokio::task::JoinSet::new();
        loop {
            while connections.try_join_next().is_some() {}
            let Ok(permit) = limit.clone().acquire_owned().await else { return };
            if server.connect().await.is_err() {
                continue;
            }
            let Ok(next) = create(&name, false) else { return };
            let client = std::mem::replace(&mut server, next);
            let games = games.clone();
            // Windows doesn't say which program is on the other end of a
            // pipe here, so it's named by the application id it gives.
            connections.spawn(async move {
                serve(client, games, None).await;
                drop(permit);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeMap;

    async fn frame(s: &mut (impl AsyncRead + AsyncWrite + Unpin), op: u32, v: Value) -> (u32, Value) {
        ipc::write_frame(s, op, &v).await.unwrap();
        ipc::read_frame(s).await.unwrap()
    }

    #[tokio::test]
    async fn a_game_session() {
        let games = Games::new(BTreeMap::from([("program:celeste".to_owned(), true)]), || {});
        let (mut game, app) = tokio::io::duplex(64 * 1024);
        let task = tokio::spawn(serve(app, games.clone(), Some("celeste".into())));

        let (op, ready) = frame(&mut game, ipc::HANDSHAKE, json!({ "v": 1, "client_id": "123" })).await;
        assert_eq!(op, ipc::FRAME);
        assert_eq!(ready["evt"], "READY");

        let set = json!({ "cmd": "SET_ACTIVITY", "nonce": "a", "args": { "pid": 1, "activity": { "details": "Chapter 3" } } });
        let (op, answer) = frame(&mut game, ipc::FRAME, set).await;
        assert_eq!((op, answer["nonce"].clone()), (ipc::FRAME, json!("a")));
        let shown = games.activities();
        assert_eq!(shown.len(), 1);
        assert_eq!((shown[0].name.as_str(), shown[0].details.as_str()), ("celeste", "Chapter 3"));

        let (op, pong) = frame(&mut game, ipc::PING, json!({ "n": 1 })).await;
        assert_eq!((op, pong), (ipc::PONG, json!({ "n": 1 })));

        // Gone with the connection.
        drop(game);
        task.await.unwrap();
        assert!(games.activities().is_empty());
    }

    #[tokio::test]
    async fn a_bad_handshake_is_closed() {
        let games = Games::new(BTreeMap::new(), || {});
        let (mut game, app) = tokio::io::duplex(1024);
        let task = tokio::spawn(serve(app, games.clone(), None));
        let (op, _) = frame(&mut game, ipc::HANDSHAKE, json!({ "v": 9 })).await;
        assert_eq!(op, ipc::CLOSE);
        task.await.unwrap();
        assert!(games.asking().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn listens_on_a_socket_for_the_same_user() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests in this binary that read the environment don't run beside this one.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", dir.path()) };
        let games = Games::new(BTreeMap::new(), || {});
        let task = tokio::spawn(listen(games.clone(), dir.path().to_owned()));
        let path = dir.path().join("discord-ipc-0");
        let mut stream = loop {
            if let Ok(s) = tokio::net::UnixStream::connect(&path).await {
                break s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let (_, ready) = frame(&mut stream, ipc::HANDSHAKE, json!({ "v": 1, "client_id": "5" })).await;
        assert_eq!(ready["evt"], "READY");
        // This test's own program is the one asked about.
        let asked = games.asking().expect("asked about");
        assert!(asked.key.starts_with("program:"), "{asked:?}");
        task.abort();
        let _ = task.await;
        assert!(!path.exists(), "the socket goes when listening stops");
    }
}
