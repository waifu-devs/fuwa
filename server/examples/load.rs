//! How many people can be online at once: a load generator for a fuwa
//! instance, run on this machine only (never against a live instance; it
//! starts its own). docs/capacity.md has what it found and how to read it.
//!
//! It starts the release `fuwa` binary as separate processes (one, or a
//! directory, two shards, a gateway and a media part), signs up people,
//! puts them in servers, follows each one's live stream on a connection of
//! its own, and makes a realistic mix of traffic: most people idle, some
//! chatting every few seconds, one channel bursting, people joining and
//! leaving, pictures going up and a call. It ramps the number of people up
//! step by step until a step breaks: sending takes over a second at p99,
//! events arrive over two seconds late at p99, a process passes its memory
//! budget, or calls fail. Each step prints a line, and a JSON summary goes
//! to `--out`.
//!
//!     cargo build --release -p fuwa-server --bin fuwa
//!     cargo run --release -p fuwa-server --example load -- --mode single
//!     cargo run --release -p fuwa-server --example load -- --mode split
//!     cargo run --release -p fuwa-server --example load -- --scenario neighbours --mode split --users 1000
//!
//! `--server-cpus 0-1` pins the instance to those CPUs (with `taskset`) so
//! the load generator's own work doesn't count against it; pin the load
//! generator itself to the others the same way.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fuwa_server::pb;

const HELP: &str = "\
load: how many people a fuwa instance holds online (docs/capacity.md)

  --mode single|split      one process, or directory + 2 shards + gateway + media (single)
  --scenario ramp|neighbours|flood
                           ramp people up until a step breaks; one hot server next to
                           quiet ones; or wrong-password sign-ins at once (ramp)
  --steps N,N,...          people online per step (100 to 8000), or sign-ins at once for flood
  --users N                people online for neighbours and flood (1000)
  --servers N              home servers, 3 or more (20)
  --square on|off          everyone also in one big shared server (on)
  --step-secs N            seconds each step or phase runs (30)
  --chat-every S           seconds between a chatter's messages (10)
  --chatters F             share of people chatting (0.25)
  --burst N                messages a second in the hot channel (100; 4000 or more: flat out)
  --churn N                joins and leaves a second on the hot server (2)
  --uploads N              picture uploads a second (0.5)
  --calls N                people in a call on the hot server (4)
  --server-cpus LIST       pin the instance to these CPUs with taskset
  --memory-mb N            memory budget per process; a step past it breaks (4096)
  --bin PATH               the fuwa binary (target/release/fuwa)
  --out PATH               JSON summary
  --keep                   keep the data folder and each process's log.txt
  --env K=V                pass an extra variable to every process";
use fuwa_server::pb::event::Payload;
use tokio::sync::Semaphore;
use tokio_stream::StreamExt;
use tonic::Request;
use tonic::transport::{Channel, Endpoint};

const PASSWORD: &str = "load test password 0123456789";
const KEY: &str = "load-cluster-key-0123456789abcdef0123456789abcdef";
const ADMIN_TOKEN: &str = "load-admin-token-0123456789abcdef0123456789";

// ---------------------------------------------------------------- options

#[derive(Clone, Debug)]
struct Options {
    mode: String,
    scenario: String,
    steps: Vec<usize>,
    users: usize,
    servers: usize,
    square: bool,
    step_secs: u64,
    chat_every: f64,
    chatters: f64,
    burst: u32,
    churn: f64,
    uploads: f64,
    calls: usize,
    server_cpus: Option<String>,
    memory_mb: u64,
    bin: PathBuf,
    out: Option<PathBuf>,
    keep: bool,
    env: Vec<(String, String)>,
}

impl Options {
    fn parse() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let mut o = Options {
            mode: "single".into(),
            scenario: "ramp".into(),
            steps: vec![100, 250, 500, 1000, 1500, 2000, 3000, 4000, 6000, 8000],
            users: 1000,
            servers: 20,
            square: true,
            step_secs: 30,
            chat_every: 10.0,
            chatters: 0.25,
            burst: 100,
            churn: 2.0,
            uploads: 0.5,
            calls: 4,
            server_cpus: None,
            memory_mb: 4096,
            bin: root.join("target/release/fuwa"),
            out: None,
            keep: false,
            env: vec![],
        };
        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            let mut value = || args.next().unwrap_or_else(|| panic!("{flag} needs a value"));
            match flag.as_str() {
                "--mode" => o.mode = value(),
                "--scenario" => o.scenario = value(),
                "--steps" => o.steps = value().split(',').map(|s| s.trim().parse().unwrap()).collect(),
                "--users" => o.users = value().parse().unwrap(),
                "--servers" => o.servers = value().parse().unwrap(),
                "--square" => o.square = value() == "on",
                "--step-secs" => o.step_secs = value().parse().unwrap(),
                "--chat-every" => o.chat_every = value().parse().unwrap(),
                "--chatters" => o.chatters = value().parse().unwrap(),
                "--burst" => o.burst = value().parse().unwrap(),
                "--churn" => o.churn = value().parse().unwrap(),
                "--uploads" => o.uploads = value().parse().unwrap(),
                "--calls" => o.calls = value().parse().unwrap(),
                "--server-cpus" => o.server_cpus = Some(value()),
                "--memory-mb" => o.memory_mb = value().parse().unwrap(),
                "--bin" => o.bin = value().into(),
                "--out" => o.out = Some(value().into()),
                "--keep" => o.keep = true,
                "--env" => {
                    let kv = value();
                    let (k, v) = kv.split_once('=').expect("--env KEY=VALUE");
                    o.env.push((k.into(), v.into()));
                }
                "--help" | "-h" => {
                    println!("{HELP}");
                    std::process::exit(0);
                }
                other => panic!("unknown flag {other} (--help lists them)"),
            }
        }
        assert!(matches!(o.mode.as_str(), "single" | "split"), "--mode single or split");
        assert!(o.servers >= 3, "--servers 3 or more");
        o
    }
}

// ---------------------------------------------------------------- numbers

/// Durations in buckets an eighth of a power of two wide (about 9%), up to
/// 2^30 µs. Lock-free, so thousands of tasks record into it at once.
struct Hist {
    buckets: Vec<AtomicU64>,
}

impl Hist {
    fn new() -> Self {
        Self { buckets: (0..256).map(|_| AtomicU64::new(0)).collect() }
    }

    fn record(&self, d: Duration) {
        let us = d.as_micros().max(1) as f64;
        let i = ((us.log2() * 8.0) as usize).min(255);
        self.buckets[i].fetch_add(1, Ordering::Relaxed);
    }

    fn count(&self) -> u64 {
        self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).sum()
    }

    /// The quantile in milliseconds (the bucket's upper edge), or 0 with nothing recorded.
    fn quantile(&self, q: f64) -> f64 {
        let total = self.count();
        if total == 0 {
            return 0.0;
        }
        let want = ((total as f64) * q).ceil() as u64;
        let mut seen = 0;
        for (i, b) in self.buckets.iter().enumerate() {
            seen += b.load(Ordering::Relaxed);
            if seen >= want {
                return 2f64.powf((i + 1) as f64 / 8.0) / 1000.0;
            }
        }
        f64::INFINITY
    }
}

/// Where a server stands relative to the hot one, for the neighbours scenario.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Hot = 0,
    SameShard = 1,
    OtherShard = 2,
    Square = 3,
}

const CLASSES: [&str; 4] = ["hot", "same_shard", "other_shard", "square"];

/// What happened during one step.
struct Window {
    send: Vec<Hist>,
    lag: Vec<Hist>,
    ready: Hist,
    upload: Hist,
    join: Hist,
    sent: AtomicU64,
    delivered: AtomicU64,
    voice_frames: AtomicU64,
    errors: Mutex<BTreeMap<String, u64>>,
}

impl Window {
    fn new() -> Self {
        Self {
            send: (0..4).map(|_| Hist::new()).collect(),
            lag: (0..4).map(|_| Hist::new()).collect(),
            ready: Hist::new(),
            upload: Hist::new(),
            join: Hist::new(),
            sent: AtomicU64::new(0),
            delivered: AtomicU64::new(0),
            voice_frames: AtomicU64::new(0),
            errors: Mutex::new(BTreeMap::new()),
        }
    }

    fn error(&self, what: &str, status: &tonic::Status) {
        let key = format!("{what}:{:?}", status.code());
        *self.errors.lock().unwrap().entry(key).or_default() += 1;
    }

    fn errors_total(&self) -> u64 {
        self.errors.lock().unwrap().values().sum()
    }
}

struct Shared {
    window: RwLock<Arc<Window>>,
    classes: RwLock<HashMap<String, Class>>,
    stop: AtomicBool,
    online: AtomicUsize,
}

impl Shared {
    fn window(&self) -> Arc<Window> {
        self.window.read().unwrap().clone()
    }

    fn class(&self, server_id: &str) -> Class {
        self.classes.read().unwrap().get(server_id).copied().unwrap_or(Class::OtherShard)
    }
}

fn now_us() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() as u64
}

// ---------------------------------------------------------------- processes

struct Proc {
    name: String,
    child: Child,
    last_cpu: u64,
    rss_max_kb: u64,
    cpu_ticks: u64,
    fds_max: usize,
}

impl Proc {
    fn pid(&self) -> u32 {
        self.child.id()
    }
}

fn read_rss_kb(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn read_cpu_ticks(pid: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    // Fields after the command name, which is in parentheses and may hold spaces.
    let rest = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or_default();
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let get = |i: usize| fields.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    // utime and stime are fields 14 and 15; after ")" they are at 11 and 12.
    get(11) + get(12)
}

fn count_fds(pid: u32) -> usize {
    std::fs::read_dir(format!("/proc/{pid}/fd")).map(|d| d.count()).unwrap_or(0)
}

struct Instance {
    url: String,
    procs: Vec<Proc>,
    dir: PathBuf,
    /// Each shard's data folder, by its id, to see where servers landed.
    shard_dirs: Vec<(String, PathBuf)>,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn spawn(o: &Options, name: &str, dir: &Path, vars: &[(&str, String)]) -> Proc {
    std::fs::create_dir_all(dir).unwrap();
    let log = std::fs::File::create(dir.join("log.txt")).unwrap();
    let mut command = match &o.server_cpus {
        Some(cpus) => {
            let mut c = Command::new("taskset");
            c.arg("-c").arg(cpus).arg(&o.bin);
            c
        }
        None => Command::new(&o.bin),
    };
    command
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("RUST_LOG", "warn")
        .env("FUWA_DATA_PATH", dir)
        .env("FUWA_HOST", "127.0.0.1")
        .env("FUWA_TELEMETRY", "off")
        .env("FUWA_ADMIN_TOKEN", ADMIN_TOKEN)
        .env("FUWA_LOCAL_ACCOUNTS", "open")
        .env("FUWA_SERVER_CREATION", "everyone")
        .env("FUWA_CALL_RECORDINGS", "off")
        .stdout(Stdio::null())
        .stderr(log);
    for (k, v) in vars {
        command.env(k, v);
    }
    for (k, v) in &o.env {
        command.env(k, v);
    }
    let child = command.spawn().unwrap_or_else(|e| panic!("couldn't start {}: {e}", o.bin.display()));
    Proc { name: name.into(), child, last_cpu: 0, rss_max_kb: 0, cpu_ticks: 0, fds_max: 0 }
}

async fn wait_healthy(url: &str) {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for _ in 0..600 {
        if let Ok(r) = client.get(format!("{url}/healthz")).send().await
            && r.status().is_success()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{url} never came up");
}

async fn start(o: &Options) -> Instance {
    let dir = std::env::temp_dir().join(format!("fuwa-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    if o.mode == "single" {
        let port = free_port();
        let url = format!("http://127.0.0.1:{port}");
        let media_port = free_port();
        let p = spawn(
            o,
            "all",
            &dir.join("all"),
            &[
                ("FUWA_PORT", port.to_string()),
                ("FUWA_PUBLIC_URL", url.clone()),
                ("FUWA_MEDIA_PORT", media_port.to_string()),
                ("FUWA_MEDIA_ADDRESSES", "127.0.0.1".into()),
            ],
        );
        wait_healthy(&url).await;
        let shard_dirs = vec![("all".to_string(), dir.join("all"))];
        return Instance { url, procs: vec![p], dir, shard_dirs };
    }
    let (gp, dp, mp, mudp) = (free_port(), free_port(), free_port(), free_port());
    let gateway_url = format!("http://127.0.0.1:{gp}");
    let directory_url = format!("http://127.0.0.1:{dp}");
    let media_url = format!("http://127.0.0.1:{mp}");
    let mut procs = vec![];
    let media = spawn(
        o,
        "media",
        &dir.join("media"),
        &[
            ("FUWA_ROLE", "media".into()),
            ("FUWA_PORT", mp.to_string()),
            ("FUWA_CLUSTER_KEY", KEY.into()),
            ("FUWA_MEDIA_PORT", mudp.to_string()),
            ("FUWA_MEDIA_ADDRESSES", "127.0.0.1".into()),
        ],
    );
    procs.push(media);
    wait_healthy(&media_url).await;
    procs.push(spawn(
        o,
        "directory",
        &dir.join("directory"),
        &[
            ("FUWA_ROLE", "directory".into()),
            ("FUWA_PORT", dp.to_string()),
            ("FUWA_PUBLIC_URL", gateway_url.clone()),
            ("FUWA_CLUSTER_KEY", KEY.into()),
            ("FUWA_MEDIA_URL", media_url.clone()),
        ],
    ));
    wait_healthy(&directory_url).await;
    let mut shard_dirs = vec![];
    for i in 1..=2 {
        let port = free_port();
        let name = format!("shard-{i}");
        let sdir = dir.join(&name);
        procs.push(spawn(
            o,
            &name,
            &sdir,
            &[
                ("FUWA_ROLE", "shard".into()),
                ("FUWA_PORT", port.to_string()),
                ("FUWA_SHARD_ID", name.clone()),
                ("FUWA_DIRECTORY_URL", directory_url.clone()),
                ("FUWA_INTERNAL_URL", format!("http://127.0.0.1:{port}")),
                ("FUWA_CLUSTER_KEY", KEY.into()),
                ("FUWA_MEDIA_URL", media_url.clone()),
            ],
        ));
        wait_healthy(&format!("http://127.0.0.1:{port}")).await;
        shard_dirs.push((name, sdir));
    }
    procs.push(spawn(
        o,
        "gateway",
        &dir.join("gateway"),
        &[
            ("FUWA_ROLE", "gateway".into()),
            ("FUWA_PORT", gp.to_string()),
            ("FUWA_DIRECTORY_URL", directory_url.clone()),
            ("FUWA_CLUSTER_KEY", KEY.into()),
        ],
    ));
    wait_healthy(&gateway_url).await;
    Instance { url: gateway_url, procs, dir, shard_dirs }
}

impl Instance {
    fn shard_of(&self, server_id: &str) -> Option<String> {
        self.shard_dirs
            .iter()
            .find(|(_, d)| d.join("servers").join(format!("{server_id}.db")).exists())
            .map(|(n, _)| n.clone())
    }

    /// Samples every process once: memory, CPU since last time, open files.
    fn sample(&mut self) {
        for p in &mut self.procs {
            let pid = p.pid();
            p.rss_max_kb = p.rss_max_kb.max(read_rss_kb(pid));
            let cpu = read_cpu_ticks(pid);
            if p.last_cpu != 0 {
                p.cpu_ticks += cpu.saturating_sub(p.last_cpu);
            }
            p.last_cpu = cpu;
            p.fds_max = p.fds_max.max(count_fds(pid));
        }
    }

    fn reset_window(&mut self) {
        for p in &mut self.procs {
            p.rss_max_kb = 0;
            p.cpu_ticks = 0;
            p.fds_max = 0;
            p.last_cpu = read_cpu_ticks(p.pid());
        }
    }

    fn dead(&mut self) -> Option<String> {
        for p in &mut self.procs {
            if let Ok(Some(status)) = p.child.try_wait() {
                return Some(format!("{} exited ({status})", p.name));
            }
        }
        None
    }

    fn stop(mut self, keep: bool) {
        for p in &mut self.procs {
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
        if !keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        } else {
            eprintln!("kept {}", self.dir.display());
        }
    }
}

// ---------------------------------------------------------------- clients

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request.set_timeout(Duration::from_secs(30));
    request
}

/// One person's connection: their own TCP connection, as a browser has.
#[derive(Clone)]
struct Conn {
    channel: Channel,
    token: String,
}

impl Conn {
    fn new(url: &str) -> Self {
        let channel = Endpoint::from_shared(url.to_string())
            .unwrap()
            .connect_timeout(Duration::from_secs(10))
            .tcp_nodelay(true)
            .connect_lazy();
        Conn { channel, token: String::new() }
    }

    fn auth(&self) -> pb::auth_service_client::AuthServiceClient<Channel> {
        pb::auth_service_client::AuthServiceClient::new(self.channel.clone())
    }
    fn servers(&self) -> pb::server_service_client::ServerServiceClient<Channel> {
        pb::server_service_client::ServerServiceClient::new(self.channel.clone())
    }
    fn channels(&self) -> pb::channel_service_client::ChannelServiceClient<Channel> {
        pb::channel_service_client::ChannelServiceClient::new(self.channel.clone())
    }
    fn messages(&self) -> pb::message_service_client::MessageServiceClient<Channel> {
        pb::message_service_client::MessageServiceClient::new(self.channel.clone())
    }
    fn events(&self) -> pb::event_service_client::EventServiceClient<Channel> {
        pb::event_service_client::EventServiceClient::new(self.channel.clone())
    }
    fn media(&self) -> pb::media_service_client::MediaServiceClient<Channel> {
        pb::media_service_client::MediaServiceClient::new(self.channel.clone())
    }

    async fn sign_up(url: &str, name: &str) -> Result<Conn, tonic::Status> {
        let mut conn = Conn::new(url);
        let req = pb::SignUpRequest { username: name.into(), password: PASSWORD.into(), display_name: String::new() };
        let mut request = Request::new(req);
        request.set_timeout(Duration::from_secs(60));
        conn.token = conn.auth().sign_up(request).await?.into_inner().token;
        Ok(conn)
    }

    async fn join(&self, server_id: &str) -> Result<(), tonic::Status> {
        let req = pb::JoinServerRequest { server_id: server_id.into(), invite_code: String::new() };
        match self.servers().join_server(authed(&self.token, req)).await {
            Ok(_) => Ok(()),
            Err(s) if s.code() == tonic::Code::AlreadyExists => Ok(()),
            Err(s) => Err(s),
        }
    }

    async fn send(&self, server: &Srv, content: String) -> Result<(), tonic::Status> {
        let req = pb::SendMessageRequest {
            server_id: server.id.clone(),
            channel_id: server.text.clone(),
            content,
            ..Default::default()
        };
        self.messages().send_message(authed(&self.token, req)).await.map(|_| ())
    }
}

#[derive(Clone, Debug)]
struct Srv {
    id: String,
    text: String,
    voice: String,
}

/// Follows a person's servers until told to stop, measuring how late each
/// load-test message arrives. Follows again from the last sequence when the
/// stream ends early.
async fn follow(conn: Conn, servers: Vec<String>, shared: Arc<Shared>, ready_tx: tokio::sync::mpsc::Sender<()>) {
    let mut last: HashMap<String, Option<i64>> = servers.iter().map(|s| (s.clone(), None)).collect();
    let mut told_ready = false;
    while !shared.stop.load(Ordering::Relaxed) {
        let cursors = last
            .iter()
            .map(|(server_id, after)| pb::ServerCursor { server_id: server_id.clone(), after_sequence: *after })
            .collect();
        let started = Instant::now();
        let mut request = Request::new(pb::SubscribeRequest { servers: cursors, ..Default::default() });
        request.metadata_mut().insert("authorization", format!("Bearer {}", conn.token).parse().unwrap());
        let stream = match conn.events().subscribe(request).await {
            Ok(s) => s.into_inner(),
            Err(status) => {
                shared.window().error("subscribe", &status);
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        tokio::pin!(stream);
        let mut online = false;
        loop {
            let item = tokio::select! {
                item = stream.next() => item,
                _ = tokio::time::sleep(Duration::from_millis(500)), if shared.stop.load(Ordering::Relaxed) => None,
            };
            let Some(item) = item else { break };
            match item {
                Err(status) => {
                    shared.window().error("stream", &status);
                    break;
                }
                Ok(response) => {
                    if let Some(ready) = response.ready {
                        shared.window().ready.record(started.elapsed());
                        for head in ready.servers {
                            last.insert(head.server_id, Some(head.sequence));
                        }
                        if !online {
                            online = true;
                            shared.online.fetch_add(1, Ordering::Relaxed);
                        }
                        if !told_ready {
                            told_ready = true;
                            let _ = ready_tx.send(()).await;
                        }
                    }
                    if let Some(event) = response.event {
                        if event.sequence > 0 {
                            last.insert(event.server_id.clone(), Some(event.sequence));
                        }
                        if let Some(Payload::MessageCreated(pb::MessageCreated { message: Some(m) })) = &event.payload
                            && let Some(sent) = m.content.strip_prefix("lg:").and_then(|t| t.split(' ').next())
                            && let Ok(sent) = sent.parse::<u64>()
                        {
                            let w = shared.window();
                            w.delivered.fetch_add(1, Ordering::Relaxed);
                            let lag = Duration::from_micros(now_us().saturating_sub(sent));
                            w.lag[shared.class(&event.server_id) as usize].record(lag);
                        }
                    }
                }
            }
        }
        if online {
            shared.online.fetch_sub(1, Ordering::Relaxed);
        }
        if !shared.stop.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

/// A message stamped with when it was sent, so receivers can tell how late it is.
fn stamped() -> String {
    format!("lg:{} hello from the load test, a line about the length people type", now_us())
}

async fn send_timed(conn: &Conn, server: &Srv, shared: &Shared) {
    let started = Instant::now();
    let result = conn.send(server, stamped()).await;
    let w = shared.window();
    match result {
        Ok(()) => {
            w.send[shared.class(&server.id) as usize].record(started.elapsed());
            w.sent.fetch_add(1, Ordering::Relaxed);
        }
        Err(status) => w.error("send", &status),
    }
}

fn png(size: usize) -> Vec<u8> {
    let chunk =
        |kind: &[u8; 4], data: &[u8]| [&(data.len() as u32).to_be_bytes()[..], kind, data, &[0, 0, 0, 0]].concat();
    [
        &b"\x89PNG\r\n\x1a\n"[..],
        &chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]),
        &chunk(b"fuWa", &vec![7; size.saturating_sub(57)]),
        &chunk(b"IEND", &[]),
    ]
    .concat()
}

async fn upload(conn: &Conn, http: &reqwest::Client, url: &str, shared: &Shared) {
    let body = png(256 * 1024);
    let started = Instant::now();
    let req = pb::CreateUploadRequest {
        purpose: pb::MediaPurpose::Avatar as i32,
        content_type: "image/png".into(),
        size: body.len() as i64,
        server_id: String::new(),
    };
    let w = shared.window();
    let reserved = match conn.media().create_upload(authed(&conn.token, req)).await {
        Ok(r) => r.into_inner(),
        Err(status) => return w.error("upload", &status),
    };
    // Links are made from the public URL; the path is all that matters here.
    let path = reqwest::Url::parse(&reserved.upload_url).map(|u| u.path().to_string()).unwrap_or_default();
    match http.put(format!("{url}{path}")).body(body).send().await {
        Ok(r) if r.status().is_success() => w.upload.record(started.elapsed()),
        Ok(r) => w.error("upload", &tonic::Status::unknown(format!("http {}", r.status()))),
        Err(_) => w.error("upload", &tonic::Status::unavailable("http")),
    }
}

// ---------------------------------------------------------------- world

struct World {
    url: String,
    owner: Conn,
    servers: Vec<Srv>,
    square: Option<Srv>,
    users: Vec<Conn>,
    shared: Arc<Shared>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    signup_secs: f64,
}

impl World {
    async fn new(url: &str, o: &Options) -> World {
        let owner = Conn::sign_up(url, "owner").await.expect("the owner signs up");
        let make = async |name: String| -> Srv {
            let req = pb::CreateServerRequest { name, discoverable: true, ..Default::default() };
            let server = owner.servers().create_server(authed(&owner.token, req)).await.unwrap().into_inner();
            let id = server.server.unwrap().id;
            let req = pb::ListChannelsRequest { server_id: id.clone() };
            let channels = owner.channels().list_channels(authed(&owner.token, req)).await.unwrap().into_inner();
            let text = channels
                .channels
                .iter()
                .find(|c| c.r#type == pb::ChannelType::Text as i32)
                .expect("a new server has a text channel")
                .id
                .clone();
            let req = pb::CreateChannelRequest {
                server_id: id.clone(),
                name: "voice".into(),
                r#type: pb::ChannelType::Voice as i32,
                ..Default::default()
            };
            let voice = owner
                .channels()
                .create_channel(authed(&owner.token, req))
                .await
                .unwrap()
                .into_inner()
                .channel
                .unwrap()
                .id;
            Srv { id, text, voice }
        };
        let mut servers = vec![];
        for i in 0..o.servers {
            servers.push(make(format!("load {i}")).await);
        }
        let square = if o.square { Some(make("square".into()).await) } else { None };
        let shared = Arc::new(Shared {
            window: RwLock::new(Arc::new(Window::new())),
            classes: RwLock::new(HashMap::new()),
            stop: AtomicBool::new(false),
            online: AtomicUsize::new(0),
        });
        World { url: url.into(), owner, servers, square, users: vec![], shared, tasks: vec![], signup_secs: 0.0 }
    }

    /// Signs up people until there are `n`, each in their home server (and
    /// the square), following their servers live. Waits until every one is
    /// ready.
    async fn grow(&mut self, n: usize) -> Result<(), String> {
        let have = self.users.len();
        if n <= have {
            return Ok(());
        }
        let started = Instant::now();
        let limit = Arc::new(Semaphore::new(32));
        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel(n);
        let mut joins = vec![];
        for i in have..n {
            let (url, limit, servers, square) =
                (self.url.clone(), limit.clone(), self.servers.clone(), self.square.clone());
            joins.push(tokio::spawn(async move {
                let _permit = limit.acquire().await.unwrap();
                let conn = Conn::sign_up(&url, &format!("user{i}")).await.map_err(|s| format!("sign up: {s}"))?;
                let home = &servers[i % servers.len()];
                let mut follow = vec![home.id.clone()];
                conn.join(&home.id).await.map_err(|s| format!("join: {s}"))?;
                if let Some(square) = &square {
                    conn.join(&square.id).await.map_err(|s| format!("join square: {s}"))?;
                    follow.push(square.id.clone());
                }
                Ok::<_, String>((conn, follow))
            }));
        }
        for join in joins {
            let (conn, follow) = join.await.map_err(|e| e.to_string())??;
            let task = tokio::spawn(self::follow(conn.clone(), follow, self.shared.clone(), ready_tx.clone()));
            self.tasks.push(task);
            self.users.push(conn);
        }
        self.signup_secs += started.elapsed().as_secs_f64();
        let deadline = Instant::now() + Duration::from_secs(120 + n as u64 / 50);
        for _ in have..n {
            match tokio::time::timeout_at(deadline.into(), ready_rx.recv()).await {
                Ok(Some(())) => {}
                _ => return Err("not every stream got ready".into()),
            }
        }
        Ok(())
    }

    fn home_of(&self, i: usize) -> &Srv {
        &self.servers[i % self.servers.len()]
    }
}

/// The traffic of one step, until `stop` is set.
struct Traffic {
    stop: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Traffic {
    async fn end(self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.tasks {
            let _ = t.await;
        }
    }
}

struct Mix {
    chatters: f64,
    chat_every: f64,
    /// Messages a second into the hot server's channel; u32::MAX for as fast as it goes.
    burst: u32,
    churn: f64,
    uploads: f64,
    calls: usize,
    /// Where joins, leaves, uploads' server and the call go (servers[hot]).
    hot: usize,
}

async fn traffic(world: &World, mix: &Mix, extras: &Extras) -> Traffic {
    let stop = Arc::new(AtomicBool::new(false));
    let mut tasks = vec![];
    let shared = world.shared.clone();
    // Chatters: a share of everyone, each sending every `chat_every` seconds
    // (a fifth of their messages in the square).
    let chatters = (world.users.len() as f64 * mix.chatters).round() as usize;
    for i in 0..chatters {
        let conn = world.users[i].clone();
        let home = world.home_of(i).clone();
        let square = world.square.clone();
        let (stop, shared) = (stop.clone(), shared.clone());
        let every = Duration::from_secs_f64(mix.chat_every);
        tasks.push(tokio::spawn(async move {
            let offset = every.mul_f64((i as f64 * 0.618_034) % 1.0);
            tokio::time::sleep(offset).await;
            let mut tick = tokio::time::interval(every);
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                tick.tick().await;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                n += 1;
                let target = match &square {
                    Some(square) if (n + i as u64).is_multiple_of(5) => square,
                    _ => &home,
                };
                send_timed(&conn, target, &shared).await;
            }
        }));
    }
    // The burst: bots in the hot server's channel.
    if mix.burst > 0 {
        let hot = world.servers[mix.hot].clone();
        let bots = extras.bots.clone();
        let workers = if mix.burst == u32::MAX { bots.len() } else { bots.len().min(mix.burst as usize).max(1) };
        let per =
            if mix.burst == u32::MAX { None } else { Some(Duration::from_secs_f64(workers as f64 / mix.burst as f64)) };
        for (k, bot) in bots.into_iter().take(workers).enumerate() {
            let (stop, shared, hot) = (stop.clone(), shared.clone(), hot.clone());
            tasks.push(tokio::spawn(async move {
                // Spread over the period, so they don't all send at the same instant.
                if let Some(per) = per {
                    tokio::time::sleep(per.mul_f64(k as f64 / workers as f64)).await;
                }
                let mut tick = per.map(tokio::time::interval);
                while !stop.load(Ordering::Relaxed) {
                    if let Some(tick) = &mut tick {
                        tick.tick().await;
                    }
                    send_timed(&bot, &hot, &shared).await;
                }
            }));
        }
    }
    // Joins and leaves on the hot server, by people who aren't measured.
    if mix.churn > 0.0 {
        let (stop, shared) = (stop.clone(), shared.clone());
        let hot = world.servers[mix.hot].id.clone();
        let churners = extras.churners.clone();
        let every = Duration::from_secs_f64(1.0 / mix.churn);
        tasks.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            let mut i = 0usize;
            let mut inside = vec![false; churners.len()];
            // One join or leave in flight per churner, so a leave never overtakes its own join.
            let busy: Arc<Vec<AtomicBool>> = Arc::new(churners.iter().map(|_| AtomicBool::new(false)).collect());
            let mut running = tokio::task::JoinSet::new();
            while !stop.load(Ordering::Relaxed) {
                tick.tick().await;
                let k = i % churners.len();
                i += 1;
                if busy[k].swap(true, Ordering::AcqRel) {
                    continue;
                }
                let (conn, hot, shared, was_in) = (churners[k].clone(), hot.clone(), shared.clone(), inside[k]);
                inside[k] = !was_in;
                let busy = busy.clone();
                running.spawn(async move {
                    let started = Instant::now();
                    let result = if was_in {
                        let req = pb::LeaveServerRequest { server_id: hot };
                        conn.servers().leave_server(authed(&conn.token, req)).await.map(|_| ())
                    } else {
                        conn.join(&hot).await
                    };
                    match result {
                        Ok(()) => shared.window().join.record(started.elapsed()),
                        Err(s) => shared.window().error(if was_in { "leave" } else { "join" }, &s),
                    }
                    busy[k].store(false, Ordering::Release);
                });
                while running.try_join_next().is_some() {}
            }
            while running.join_next().await.is_some() {}
        }));
    }
    if mix.uploads > 0.0 {
        let (stop, shared) = (stop.clone(), shared.clone());
        let uploaders = extras.uploaders.clone();
        let url = world.url.clone();
        let every = Duration::from_secs_f64(1.0 / mix.uploads);
        tasks.push(tokio::spawn(async move {
            let http = reqwest::Client::builder().no_proxy().build().unwrap();
            let mut tick = tokio::time::interval(every);
            let mut i = 0usize;
            let mut running = tokio::task::JoinSet::new();
            while !stop.load(Ordering::Relaxed) {
                tick.tick().await;
                let conn = uploaders[i % uploaders.len()].clone();
                i += 1;
                let (http, url, shared) = (http.clone(), url.clone(), shared.clone());
                running.spawn(async move { upload(&conn, &http, &url, &shared).await });
                while running.try_join_next().is_some() {}
            }
            while running.join_next().await.is_some() {}
        }));
    }
    // A call: each caller speaks a 20 ms frame and hears the others.
    for caller in extras.callers.iter().take(mix.calls) {
        let (stop, shared) = (stop.clone(), shared.clone());
        let (url, token) = (world.url.clone(), caller.token.clone());
        let hot = world.servers[mix.hot].clone();
        tasks.push(tokio::spawn(async move {
            let client = match fuwa_voice::Client::connect(&url, &token).await {
                Ok(c) => c,
                Err(s) => return shared.window().error("call", &s),
            };
            let (mut heard, speaker) = match client.join(&hot.id, &hot.voice).await {
                Ok(v) => v,
                Err(s) => return shared.window().error("call", &s),
            };
            let speaking = {
                let stop = stop.clone();
                let shared = shared.clone();
                tokio::spawn(async move {
                    let mut tick = tokio::time::interval(Duration::from_millis(200));
                    while !stop.load(Ordering::Relaxed) {
                        tick.tick().await;
                        // Ten frames at a time, 200 ms of sound.
                        if let Err(s) = speaker.say((0..10).map(|_| vec![0xfc; 80])).await {
                            shared.window().error("call_say", &s);
                        }
                    }
                })
            };
            while !stop.load(Ordering::Relaxed) {
                match tokio::time::timeout(Duration::from_millis(500), heard.next()).await {
                    Ok(Ok(Some(_))) => {
                        shared.window().voice_frames.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(Ok(None)) => break,
                    Ok(Err(s)) => {
                        shared.window().error("call_hear", &s);
                        break;
                    }
                    Err(_) => {}
                }
            }
            speaking.abort();
        }));
    }
    Traffic { stop, tasks }
}

/// Accounts that make traffic but aren't among the people measured.
struct Extras {
    bots: Vec<Conn>,
    churners: Vec<Conn>,
    uploaders: Vec<Conn>,
    callers: Vec<Conn>,
}

async fn extras(world: &World, hot: usize, o: &Options) -> Extras {
    let make = async |prefix: &str, n: usize, join: bool| {
        let mut out = vec![];
        for i in 0..n {
            let conn = Conn::sign_up(&world.url, &format!("{prefix}{i}")).await.expect("an extra signs up");
            if join {
                conn.join(&world.servers[hot].id).await.expect("an extra joins");
            }
            out.push(conn);
        }
        out
    };
    Extras {
        bots: make("bot", 100, true).await,
        churners: make("churn", 40, false).await,
        uploaders: make("up", 8, false).await,
        callers: make("caller", o.calls, true).await,
    }
}

// ---------------------------------------------------------------- report

#[derive(serde::Serialize)]
struct Row {
    users: usize,
    online: usize,
    secs: f64,
    sent_per_s: f64,
    delivered_per_s: f64,
    send_p50_ms: f64,
    send_p99_ms: f64,
    lag_p50_ms: f64,
    lag_p99_ms: f64,
    lag_by_class: BTreeMap<String, (f64, f64, u64)>,
    send_by_class: BTreeMap<String, (f64, f64, u64)>,
    ready_p99_ms: f64,
    upload_p99_ms: f64,
    join_p99_ms: f64,
    voice_frames_per_s: f64,
    errors: BTreeMap<String, u64>,
    procs: Vec<ProcRow>,
    loadgen_cpu_pct: f64,
    broke: Option<String>,
}

#[derive(serde::Serialize)]
struct ProcRow {
    name: String,
    rss_mb: f64,
    cpu_pct: f64,
    fds: usize,
}

fn ticks_per_sec() -> f64 {
    100.0
}

fn summarize(n: usize, world: &World, inst: &Instance, secs: f64, self_cpu: u64, o: &Options) -> Row {
    let w = world.shared.window();
    let merged = |hs: &[Hist]| {
        let all = Hist::new();
        for h in hs {
            for (i, b) in h.buckets.iter().enumerate() {
                all.buckets[i].fetch_add(b.load(Ordering::Relaxed), Ordering::Relaxed);
            }
        }
        all
    };
    let (send, lag) = (merged(&w.send), merged(&w.lag));
    let by = |hs: &[Hist]| {
        hs.iter()
            .enumerate()
            .filter(|(_, h)| h.count() > 0)
            .map(|(i, h)| (CLASSES[i].to_string(), (h.quantile(0.5), h.quantile(0.99), h.count())))
            .collect::<BTreeMap<_, _>>()
    };
    let procs: Vec<ProcRow> = inst
        .procs
        .iter()
        .map(|p| ProcRow {
            name: p.name.clone(),
            rss_mb: p.rss_max_kb as f64 / 1024.0,
            cpu_pct: p.cpu_ticks as f64 / ticks_per_sec() / secs * 100.0,
            fds: p.fds_max,
        })
        .collect();
    let errors = w.errors.lock().unwrap().clone();
    let total_ops = w.sent.load(Ordering::Relaxed) + w.errors_total();
    let mut broke = None;
    if send.quantile(0.99) > 1000.0 {
        broke = Some(format!("send p99 {:.0} ms > 1 s", send.quantile(0.99)));
    } else if lag.quantile(0.99) > 2000.0 {
        broke = Some(format!("event lag p99 {:.0} ms > 2 s", lag.quantile(0.99)));
    } else if let Some(p) = procs.iter().find(|p| p.rss_mb > o.memory_mb as f64) {
        broke = Some(format!("{} at {:.0} MB > {} MB budget", p.name, p.rss_mb, o.memory_mb));
    } else if w.errors_total() * 1000 > total_ops.max(1) {
        broke = Some(format!("errors {:?}", errors));
    }
    Row {
        users: n,
        online: world.shared.online.load(Ordering::Relaxed),
        secs,
        sent_per_s: w.sent.load(Ordering::Relaxed) as f64 / secs,
        delivered_per_s: w.delivered.load(Ordering::Relaxed) as f64 / secs,
        send_p50_ms: send.quantile(0.5),
        send_p99_ms: send.quantile(0.99),
        lag_p50_ms: lag.quantile(0.5),
        lag_p99_ms: lag.quantile(0.99),
        lag_by_class: by(&w.lag),
        send_by_class: by(&w.send),
        ready_p99_ms: w.ready.quantile(0.99),
        upload_p99_ms: w.upload.quantile(0.99),
        join_p99_ms: w.join.quantile(0.99),
        voice_frames_per_s: w.voice_frames.load(Ordering::Relaxed) as f64 / secs,
        errors,
        procs,
        loadgen_cpu_pct: self_cpu as f64 / ticks_per_sec() / secs * 100.0,
        broke,
    }
}

fn print_row(label: &str, r: &Row) {
    let procs: Vec<String> =
        r.procs.iter().map(|p| format!("{} {:.0}MB {:.0}% {}fd", p.name, p.rss_mb, p.cpu_pct, p.fds)).collect();
    println!(
        "{label} users {} online {} | sent {:.0}/s delivered {:.0}/s | send p50 {:.1} p99 {:.1} ms | lag p50 {:.1} p99 {:.1} ms | ready p99 {:.0} upload p99 {:.0} join p99 {:.0} ms | voice {:.0} f/s | {} | loadgen {:.0}% | errors {:?}{}",
        r.users,
        r.online,
        r.sent_per_s,
        r.delivered_per_s,
        r.send_p50_ms,
        r.send_p99_ms,
        r.lag_p50_ms,
        r.lag_p99_ms,
        r.ready_p99_ms,
        r.upload_p99_ms,
        r.join_p99_ms,
        r.voice_frames_per_s,
        procs.join(", "),
        r.loadgen_cpu_pct,
        r.errors,
        r.broke.as_ref().map(|b| format!(" | BROKE: {b}")).unwrap_or_default()
    );
    if r.lag_by_class.len() > 1 {
        println!("    lag by class {:?}", r.lag_by_class);
        println!("    send by class {:?}", r.send_by_class);
    }
}

/// Runs the mix for `secs`, sampling the processes every second.
async fn measure(world: &World, inst: &mut Instance, mix: &Mix, extras: &Extras, secs: u64) -> (f64, u64) {
    *world.shared.window.write().unwrap() = Arc::new(Window::new());
    inst.reset_window();
    let self_start = read_cpu_ticks(std::process::id());
    let traffic = traffic(world, mix, extras).await;
    let started = Instant::now();
    for _ in 0..secs {
        tokio::time::sleep(Duration::from_secs(1)).await;
        inst.sample();
    }
    let elapsed = started.elapsed().as_secs_f64();
    traffic.end().await;
    (elapsed, read_cpu_ticks(std::process::id()) - self_start)
}

fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string()))
        .unwrap_or_default()
}

#[tokio::main]
async fn main() {
    let o = Options::parse();
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let mem =
        std::fs::read_to_string("/proc/meminfo").unwrap_or_default().lines().next().unwrap_or_default().to_string();
    println!(
        "fuwa load: mode {} scenario {} | machine {} CPUs ({}), {} | instance CPUs {} | loadgen CPUs {}",
        o.mode,
        o.scenario,
        num_cpus_online(),
        cpu_model(),
        mem.split_whitespace().skip(1).collect::<Vec<_>>().join(" "),
        o.server_cpus.as_deref().unwrap_or("all"),
        cpus
    );
    let mut inst = start(&o).await;
    let mut world = World::new(&inst.url, &o).await;
    let hot = 0;
    {
        let mut classes = world.shared.classes.write().unwrap();
        let hot_shard = inst.shard_of(&world.servers[hot].id);
        for (i, s) in world.servers.iter().enumerate() {
            let class = if i == hot {
                Class::Hot
            } else if inst.shard_of(&s.id) == hot_shard {
                Class::SameShard
            } else {
                Class::OtherShard
            };
            classes.insert(s.id.clone(), class);
        }
        if let Some(square) = &world.square {
            classes.insert(square.id.clone(), Class::Square);
        }
        let placement: Vec<String> =
            world.servers.iter().map(|s| inst.shard_of(&s.id).unwrap_or_else(|| "?".into())).collect();
        println!("placement (server -> shard): {placement:?}; hot is server 0");
    }
    let extras = extras(&world, hot, &o).await;
    let mut rows = vec![];

    if o.scenario == "flood" {
        // Sign-in attempts at once, each a different name with a wrong
        // password, while a few people chat: what a password-guessing flood
        // does to memory and to everyone else.
        world.grow(o.users).await.expect("everyone gets online");
        let calm =
            Mix { chatters: o.chatters, chat_every: o.chat_every, burst: 0, churn: 0.0, uploads: 0.0, calls: 0, hot };
        let flood = o.steps[0];
        let url = world.url.clone();
        let shared = world.shared.clone();
        let flooding = tokio::spawn(async move {
            let mut running = tokio::task::JoinSet::new();
            for i in 0..flood {
                let (url, shared) = (url.clone(), shared.clone());
                running.spawn(async move {
                    let conn = Conn::new(&url);
                    let req = pb::SignInRequest { username: format!("nobody{i}"), password: "wrong".into() };
                    let mut request = Request::new(req);
                    request.set_timeout(Duration::from_secs(60));
                    match conn.auth().sign_in(request).await {
                        Err(s) if s.code() == tonic::Code::Unauthenticated => {}
                        Err(s) => shared.window().error("sign_in", &s),
                        Ok(_) => {}
                    }
                });
            }
            while running.join_next().await.is_some() {}
        });
        let (secs, cpu) = measure(&world, &mut inst, &calm, &extras, o.step_secs).await;
        let _ = flooding.await;
        let row = summarize(o.users, &world, &inst, secs, cpu, &o);
        print_row(&format!("flood {flood}"), &row);
        if let Some(dead) = inst.dead() {
            println!("BROKE: {dead}");
        }
        rows.push((format!("flood {flood}"), row));
    } else if o.scenario == "neighbours" {
        world.grow(o.users).await.expect("everyone gets online");
        println!("{} online (sign-ups took {:.1} s)", o.users, world.signup_secs);
        let calm =
            Mix { chatters: o.chatters, chat_every: o.chat_every, burst: 0, churn: 0.0, uploads: 0.0, calls: 0, hot };
        let (secs, cpu) = measure(&world, &mut inst, &calm, &extras, o.step_secs).await;
        let row = summarize(o.users, &world, &inst, secs, cpu, &o);
        print_row("calm", &row);
        rows.push(("calm".to_string(), row));
        for (label, mix) in [
            ("burst", Mix { burst: u32::MAX, ..Mix { ..clone_mix(&calm) } }),
            ("joins", Mix { churn: 200.0, ..clone_mix(&calm) }),
            ("uploads", Mix { uploads: 20.0, ..clone_mix(&calm) }),
            ("call", Mix { calls: o.calls, ..clone_mix(&calm) }),
            ("everything", Mix { burst: u32::MAX, churn: 200.0, uploads: 20.0, calls: o.calls, ..clone_mix(&calm) }),
        ] {
            let (secs, cpu) = measure(&world, &mut inst, &mix, &extras, o.step_secs).await;
            let row = summarize(o.users, &world, &inst, secs, cpu, &o);
            print_row(label, &row);
            rows.push((label.to_string(), row));
            if let Some(dead) = inst.dead() {
                println!("BROKE: {dead}");
                break;
            }
        }
    } else {
        let mix = Mix {
            chatters: o.chatters,
            chat_every: o.chat_every,
            burst: o.burst,
            churn: o.churn,
            uploads: o.uploads,
            calls: o.calls,
            hot,
        };
        for &n in &o.steps {
            let grown = world.grow(n).await;
            if let Some(dead) = inst.dead() {
                println!("BROKE at {n}: {dead}");
                break;
            }
            if let Err(err) = grown {
                println!(
                    "BROKE at {n} while getting everyone online: {err} ({} online)",
                    world.shared.online.load(Ordering::Relaxed)
                );
                println!("    errors {:?}", world.shared.window().errors.lock().unwrap());
                break;
            }
            let (secs, cpu) = measure(&world, &mut inst, &mix, &extras, o.step_secs).await;
            let row = summarize(n, &world, &inst, secs, cpu, &o);
            print_row("step", &row);
            let broke = row.broke.is_some();
            rows.push((format!("{n}"), row));
            if let Some(dead) = inst.dead() {
                println!("BROKE at {n}: {dead}");
                break;
            }
            if broke {
                break;
            }
        }
        println!("sign-ups: {} people in {:.1} s", world.users.len(), world.signup_secs);
    }

    if let Some(out) = &o.out {
        let json = serde_json::json!({
            "mode": o.mode,
            "scenario": o.scenario,
            "machine": { "cpus": num_cpus_online(), "model": cpu_model(), "memory": mem },
            "instance_cpus": o.server_cpus,
            "servers": o.servers,
            "square": o.square,
            "rows": rows.iter().map(|(l, r)| serde_json::json!({ "label": l, "row": r })).collect::<Vec<_>>(),
        });
        std::fs::write(out, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }
    world.shared.stop.store(true, Ordering::Relaxed);
    drop(world.owner);
    inst.stop(o.keep);
    std::process::exit(0);
}

fn clone_mix(m: &Mix) -> Mix {
    Mix {
        chatters: m.chatters,
        chat_every: m.chat_every,
        burst: m.burst,
        churn: m.churn,
        uploads: m.uploads,
        calls: m.calls,
        hot: m.hot,
    }
}

fn num_cpus_online() -> usize {
    std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default().lines().filter(|l| l.starts_with("processor")).count()
}
