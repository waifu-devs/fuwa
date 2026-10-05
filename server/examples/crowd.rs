//! One huge server with many people online at once: a load generator for
//! what a crowd costs. It starts its own instance, or with `--url` uses a
//! test instance of your own, such as one in a Railway project of its own
//! (deploy/crowd.Dockerfile builds both); never a live one. `load.rs` measures many people across many
//! servers; this one puts everyone in the same server, where every message,
//! presence change and permission change reaches everyone.
//!
//!     cargo build --release -p fuwa-server --bin fuwa --example crowd
//!     target/release/examples/crowd --people 20000 --data /tmp/crowd-20k
//!
//! Signing everyone up is slow (each password takes a real hash), so
//! `--data` keeps the instance's folder and the tokens, and a later run with
//! the same folder and `--people` up to what's there skips it.
//!
//! Each phase prints one line, and `--out` gets them all as JSON:
//! - connect: everyone opens their events stream at once (up to `--storm`
//!   opening together); memory per stream, time until all are live.
//! - tab: everyone also opens what a web tab opens (direct messages,
//!   friends, presence), each measured on its own.
//! - online: everyone says they're online, as an app does when it starts;
//!   how many presence updates that sends, and how long it takes.
//! - idle: nothing happens for `--idle-secs`; what heartbeats cost.
//! - fanout: one channel gets `--rates` messages a second in turn; how late
//!   they reach everyone.
//! - access: a channel's topic changes, which makes every stream work out
//!   again what its member can see.
//! - members: listing the server's members, once and 50 at once.
//! - reconnect: every stream drops and follows again from its last event,
//!   as after a restart.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::{Buf, Bytes};
use fuwa_server::pb;
use tokio::sync::{Notify, Semaphore};
use tokio_stream::StreamExt;
use tonic::Request;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::transport::{Channel, Endpoint};

const HELP: &str = "\
crowd: what one huge server with everyone online costs

  --people N          people in the server, all online (10000)
  --mode single|split one process, or directory + shard + gateway (single)
  --phases LIST       connect,tab,online,idle,fanout,access,members,reconnect
  --rates LIST        messages a second for fanout (1,5,20,50)
  --step-secs N       seconds per fanout rate (20)
  --idle-secs N       seconds of the idle phase (60)
  --storm N           streams opening at once (500)
  --per-conn N        streams per HTTP/2 connection (100; 1 is one each, like browsers over HTTP/1.1)
  --server-cpus LIST  pin the instance with taskset
  --data DIR          keep the instance's folder and tokens here, and reuse them
  --url URL           use an instance that's already running (a test one, never a live
                      one) instead of starting one; memory and CPU then come from its host
  --bin PATH          the fuwa binary (target/release/fuwa)
  --out PATH          JSON lines
  --env K=V           an extra variable for every process";

const PASSWORD: &str = "crowd test password 0123456789";
const KEY: &str = "crowd-cluster-key-0123456789abcdef0123456789abcdef";
const ADMIN_TOKEN: &str = "crowd-admin-token-0123456789abcdef0123456789";

#[derive(Clone, Debug)]
struct Options {
    people: usize,
    mode: String,
    phases: Vec<String>,
    rates: Vec<f64>,
    step_secs: u64,
    idle_secs: u64,
    storm: usize,
    per_conn: usize,
    server_cpus: Option<String>,
    data: Option<PathBuf>,
    url: Option<String>,
    bin: PathBuf,
    out: Option<PathBuf>,
    env: Vec<(String, String)>,
}

impl Options {
    fn parse() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let mut o = Options {
            people: 10_000,
            mode: "single".into(),
            phases: "connect,tab,online,idle,fanout,access,members,reconnect".split(',').map(String::from).collect(),
            rates: vec![1.0, 5.0, 20.0, 50.0],
            step_secs: 20,
            idle_secs: 60,
            storm: 500,
            per_conn: 100,
            server_cpus: None,
            data: None,
            url: None,
            bin: root.join("target/release/fuwa"),
            out: None,
            env: vec![],
        };
        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            let mut value = || args.next().unwrap_or_else(|| panic!("{flag} needs a value"));
            match flag.as_str() {
                "--people" => o.people = value().parse().unwrap(),
                "--mode" => o.mode = value(),
                "--phases" => o.phases = value().split(',').map(|s| s.trim().to_string()).collect(),
                "--rates" => o.rates = value().split(',').map(|s| s.trim().parse().unwrap()).collect(),
                "--step-secs" => o.step_secs = value().parse().unwrap(),
                "--idle-secs" => o.idle_secs = value().parse().unwrap(),
                "--storm" => o.storm = value().parse().unwrap(),
                "--per-conn" => o.per_conn = value().parse::<usize>().unwrap().max(1),
                "--server-cpus" => o.server_cpus = Some(value()),
                "--data" => o.data = Some(value().into()),
                "--url" => o.url = Some(value()),
                "--bin" => o.bin = value().into(),
                "--out" => o.out = Some(value().into()),
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
        o
    }

    fn has(&self, phase: &str) -> bool {
        self.phases.iter().any(|p| p == phase)
    }
}

// ---------------------------------------------------------------- numbers

/// Durations in buckets an eighth of a power of two wide, lock-free.
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

    /// Milliseconds, the bucket's upper edge.
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

fn now_us() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() as u64
}

/// What every stream counts into, swapped for each phase.
struct Tally {
    frames: AtomicU64,
    bytes: AtomicU64,
    /// Messages stamped "lg:<µs>" that arrived, and how late.
    lag: Hist,
    delivered: AtomicU64,
    /// Topic changes stamped "tp:<µs>" that arrived, and how late.
    topic_lag: Hist,
    topics: AtomicU64,
    presences: AtomicU64,
    errors: Mutex<BTreeMap<String, u64>>,
}

impl Tally {
    fn new() -> Self {
        Self {
            frames: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            lag: Hist::new(),
            delivered: AtomicU64::new(0),
            topic_lag: Hist::new(),
            topics: AtomicU64::new(0),
            presences: AtomicU64::new(0),
            errors: Mutex::new(BTreeMap::new()),
        }
    }

    fn error(&self, what: &str, status: &tonic::Status) {
        let key = format!("{what}:{:?}", status.code());
        *self.errors.lock().unwrap().entry(key).or_default() += 1;
    }

    fn errors(&self) -> BTreeMap<String, u64> {
        self.errors.lock().unwrap().clone()
    }
}

struct Shared {
    tally: Mutex<Arc<Tally>>,
    /// Bumped to make every events stream drop and follow again.
    generation: AtomicU64,
    regen: Notify,
    live: AtomicUsize,
    ready: Hist,
}

impl Shared {
    fn tally(&self) -> Arc<Tally> {
        self.tally.lock().unwrap().clone()
    }

    fn fresh(&self) -> Arc<Tally> {
        let t = Arc::new(Tally::new());
        *self.tally.lock().unwrap() = t.clone();
        t
    }
}

// ---------------------------------------------------------------- processes

struct Proc {
    name: String,
    child: Child,
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
    let rest = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or_default();
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let get = |i: usize| fields.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    get(11) + get(12)
}

struct Instance {
    url: String,
    procs: Vec<Proc>,
}

/// Memory (MB) and CPU ticks of each process, by name.
#[derive(Clone)]
struct Sample {
    at: Instant,
    rss_kb: Vec<(String, u64)>,
    ticks: Vec<(String, u64)>,
}

impl Instance {
    fn sample(&self) -> Sample {
        Sample {
            at: Instant::now(),
            rss_kb: self.procs.iter().map(|p| (p.name.clone(), read_rss_kb(p.child.id()))).collect(),
            ticks: self.procs.iter().map(|p| (p.name.clone(), read_cpu_ticks(p.child.id()))).collect(),
        }
    }

    fn rss_total_kb(&self) -> u64 {
        self.procs.iter().map(|p| read_rss_kb(p.child.id())).sum()
    }

    fn stop(mut self) {
        for p in &mut self.procs {
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
    }
}

/// CPU used between two samples, as a percentage of one core, per process.
fn cpu_between(a: &Sample, b: &Sample) -> serde_json::Value {
    let secs = b.at.duration_since(a.at).as_secs_f64().max(0.001);
    let mut out = serde_json::Map::new();
    for ((name, t0), (_, t1)) in a.ticks.iter().zip(&b.ticks) {
        // Ticks are hundredths of a second.
        let pct = (t1.saturating_sub(*t0)) as f64 / secs;
        out.insert(name.clone(), serde_json::json!((pct * 10.0).round() / 10.0));
    }
    serde_json::Value::Object(out)
}

fn rss_mb(s: &Sample) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (name, kb) in &s.rss_kb {
        out.insert(name.clone(), serde_json::json!(kb / 1024));
    }
    serde_json::Value::Object(out)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn spawn(o: &Options, name: &str, dir: &Path, vars: &[(&str, String)]) -> Proc {
    std::fs::create_dir_all(dir).unwrap();
    let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("log.txt")).unwrap();
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
        // Everyone here is one tab, but the harness opens them fast.
        .env("FUWA_STREAMS_PER_ACCOUNT", "unlimited")
        .stdout(Stdio::null())
        .stderr(log);
    for (k, v) in vars {
        command.env(k, v);
    }
    for (k, v) in &o.env {
        command.env(k, v);
    }
    let child = command.spawn().unwrap_or_else(|e| panic!("couldn't start {}: {e}", o.bin.display()));
    Proc { name: name.into(), child }
}

async fn wait_healthy(url: &str) {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for _ in 0..1200 {
        if let Ok(r) = client.get(format!("{url}/healthz")).send().await
            && r.status().is_success()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{url} never came up");
}

async fn start(o: &Options, dir: &Path) -> Instance {
    if let Some(url) = &o.url {
        wait_healthy(url).await;
        return Instance { url: url.clone(), procs: vec![] };
    }
    if o.mode == "single" {
        let port = free_port();
        let url = format!("http://127.0.0.1:{port}");
        let p = spawn(
            o,
            "all",
            &dir.join("all"),
            &[
                ("FUWA_PORT", port.to_string()),
                ("FUWA_PUBLIC_URL", url.clone()),
                ("FUWA_MEDIA_PORT", free_port().to_string()),
                ("FUWA_MEDIA_ADDRESSES", "127.0.0.1".into()),
            ],
        );
        wait_healthy(&url).await;
        return Instance { url, procs: vec![p] };
    }
    let (gp, dp, sp) = (free_port(), free_port(), free_port());
    let gateway_url = format!("http://127.0.0.1:{gp}");
    let directory_url = format!("http://127.0.0.1:{dp}");
    let mut procs = vec![];
    procs.push(spawn(
        o,
        "directory",
        &dir.join("directory"),
        &[
            ("FUWA_ROLE", "directory".into()),
            ("FUWA_PORT", dp.to_string()),
            ("FUWA_PUBLIC_URL", gateway_url.clone()),
            ("FUWA_CLUSTER_KEY", KEY.into()),
        ],
    ));
    wait_healthy(&directory_url).await;
    procs.push(spawn(
        o,
        "shard",
        &dir.join("shard-1"),
        &[
            ("FUWA_ROLE", "shard".into()),
            ("FUWA_PORT", sp.to_string()),
            ("FUWA_SHARD_ID", "shard-1".into()),
            ("FUWA_DIRECTORY_URL", directory_url.clone()),
            ("FUWA_INTERNAL_URL", format!("http://127.0.0.1:{sp}")),
            ("FUWA_CLUSTER_KEY", KEY.into()),
        ],
    ));
    wait_healthy(&format!("http://127.0.0.1:{sp}")).await;
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
    Instance { url: gateway_url, procs }
}

// ---------------------------------------------------------------- wire

/// Sends a request as protobuf and hands back each response as it came, not
/// decoded, so a crowd of streams costs this side little.
struct Raw<T>(PhantomData<T>);

impl<T> Default for Raw<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

struct RawEncoder<T>(PhantomData<T>);
struct RawDecoder;

impl<T: prost::Message + Send + 'static> Codec for Raw<T> {
    type Encode = T;
    type Decode = Bytes;
    type Encoder = RawEncoder<T>;
    type Decoder = RawDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        RawEncoder(PhantomData)
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawDecoder
    }
}

impl<T: prost::Message> Encoder for RawEncoder<T> {
    type Item = T;
    type Error = tonic::Status;

    fn encode(&mut self, item: T, dst: &mut EncodeBuf<'_>) -> Result<(), tonic::Status> {
        item.encode(dst).map_err(|e| tonic::Status::internal(e.to_string()))
    }
}

impl Decoder for RawDecoder {
    type Item = Bytes;
    type Error = tonic::Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Bytes>, tonic::Status> {
        Ok(Some(src.copy_to_bytes(src.remaining())))
    }
}

async fn open_raw<T: prost::Message + Send + Sync + 'static>(
    channel: &Channel,
    token: &str,
    path: &'static str,
    message: T,
) -> Result<tonic::Streaming<Bytes>, tonic::Status> {
    let mut grpc = tonic::client::Grpc::new(channel.clone());
    grpc.ready().await.map_err(|e| tonic::Status::unavailable(e.to_string()))?;
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    let path = http::uri::PathAndQuery::from_static(path);
    Ok(grpc.server_streaming(request, path, Raw::<T>::default()).await?.into_inner())
}

fn varint(buf: &mut &[u8]) -> Option<u64> {
    let mut out = 0u64;
    for shift in (0..64).step_by(7) {
        let (&b, rest) = buf.split_first()?;
        *buf = rest;
        out |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(out);
        }
    }
    None
}

/// The fields of one protobuf message, as (field number, varint value or
/// the bytes of a length-delimited one).
fn fields(mut buf: &[u8]) -> Vec<(u64, Result<u64, &[u8]>)> {
    let mut out = vec![];
    while !buf.is_empty() {
        let Some(key) = varint(&mut buf) else { break };
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => match varint(&mut buf) {
                Some(v) => out.push((field, Ok(v))),
                None => break,
            },
            2 => {
                let Some(len) = varint(&mut buf) else { break };
                let len = len as usize;
                if len > buf.len() {
                    break;
                }
                out.push((field, Err(&buf[..len])));
                buf = &buf[len..];
            }
            1 => buf = buf.get(8..).unwrap_or_default(),
            5 => buf = buf.get(4..).unwrap_or_default(),
            _ => break,
        }
    }
    out
}

/// A stamp like "lg:1234567 " in a frame: the µs it was made.
fn stamp(frame: &[u8], tag: &[u8; 3]) -> Option<u64> {
    let at = frame.windows(3).position(|w| w == tag)?;
    let digits: Vec<u8> = frame[at + 3..].iter().take_while(|b| b.is_ascii_digit()).copied().collect();
    std::str::from_utf8(&digits).ok()?.parse().ok()
}

// ---------------------------------------------------------------- people

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request.set_timeout(Duration::from_secs(120));
    request
}

fn connect(url: &str) -> Channel {
    Endpoint::from_shared(url.to_string())
        .unwrap()
        .connect_timeout(Duration::from_secs(20))
        .tcp_nodelay(true)
        .http2_keep_alive_interval(Duration::from_secs(30))
        .initial_stream_window_size(Some(256 * 1024))
        .connect_lazy()
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Setup {
    server_id: String,
    text: String,
    quiet: String,
    owner: String,
    tokens: Vec<String>,
}

async fn sign_up(channel: &Channel, name: &str) -> Result<String, tonic::Status> {
    let mut auth = pb::auth_service_client::AuthServiceClient::new(channel.clone());
    let req = pb::SignUpRequest { username: name.into(), password: PASSWORD.into(), display_name: String::new() };
    let mut request = Request::new(req);
    request.set_timeout(Duration::from_secs(120));
    Ok(auth.sign_up(request).await?.into_inner().token)
}

async fn setup(url: &str, have: Option<Setup>, people: usize) -> Setup {
    let channel = connect(url);
    let mut setup = match have {
        Some(s) => s,
        None => {
            let owner = sign_up(&channel, "owner").await.expect("the owner signs up");
            let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
            let req = pb::CreateServerRequest { name: "plaza".into(), discoverable: true, ..Default::default() };
            let server = servers.create_server(authed(&owner, req)).await.unwrap().into_inner().server.unwrap();
            let mut channels = pb::channel_service_client::ChannelServiceClient::new(channel.clone());
            let req = pb::ListChannelsRequest { server_id: server.id.clone() };
            let list = channels.list_channels(authed(&owner, req)).await.unwrap().into_inner().channels;
            let text = list.iter().find(|c| c.r#type == pb::ChannelType::Text as i32).unwrap().id.clone();
            let req = pb::CreateChannelRequest {
                server_id: server.id.clone(),
                name: "quiet".into(),
                r#type: pb::ChannelType::Text as i32,
                ..Default::default()
            };
            let quiet = channels.create_channel(authed(&owner, req)).await.unwrap().into_inner().channel.unwrap().id;
            Setup { server_id: server.id, text, quiet, owner, tokens: vec![] }
        }
    };
    if setup.tokens.len() >= people {
        return setup;
    }
    let started = Instant::now();
    let limit = Arc::new(Semaphore::new(16));
    let done = Arc::new(AtomicUsize::new(0));
    let mut joins = vec![];
    for i in setup.tokens.len()..people {
        let (limit, channel, server_id, done) = (limit.clone(), channel.clone(), setup.server_id.clone(), done.clone());
        joins.push(tokio::spawn(async move {
            let _permit = limit.acquire().await.unwrap();
            let token = loop {
                match sign_up(&channel, &format!("crowd{i}")).await {
                    Ok(token) => break token,
                    Err(s) if s.code() == tonic::Code::ResourceExhausted || s.code() == tonic::Code::Unavailable => {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    Err(s) => panic!("sign up: {s}"),
                }
            };
            let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
            loop {
                let req = pb::JoinServerRequest { server_id: server_id.clone(), invite_code: String::new() };
                match servers.join_server(authed(&token, req)).await {
                    Ok(_) => break,
                    Err(s) if s.code() == tonic::Code::AlreadyExists => break,
                    Err(s) if s.code() == tonic::Code::ResourceExhausted || s.code() == tonic::Code::Unavailable => {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    Err(s) => panic!("join: {s}"),
                }
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 2000 == 0 {
                eprintln!("  {n} signed up and joined");
            }
            token
        }));
    }
    for join in joins {
        setup.tokens.push(join.await.unwrap());
    }
    eprintln!("signed up {} people in {:.0}s", people, started.elapsed().as_secs_f64());
    setup
}

/// Follows the server for one person until the end, again from its last
/// event whenever the generation moves on or the stream ends.
async fn follow(
    channel: Channel,
    token: String,
    server_id: String,
    shared: Arc<Shared>,
    storm: Arc<Semaphore>,
    ready_tx: tokio::sync::mpsc::UnboundedSender<()>,
) {
    let mut last: Option<i64> = None;
    loop {
        let generation = shared.generation.load(Ordering::Acquire);
        let permit = storm.acquire().await.unwrap();
        let started = Instant::now();
        let cursor = pb::ServerCursor { server_id: server_id.clone(), after_sequence: last };
        let request = pb::SubscribeRequest { servers: vec![cursor], ..Default::default() };
        let stream = match open_raw(&channel, &token, "/fuwa.v1.EventService/Subscribe", request).await {
            Ok(s) => s,
            Err(status) => {
                drop(permit);
                shared.tally().error("subscribe", &status);
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let mut permit = Some(permit);
        tokio::pin!(stream);
        let mut live = false;
        loop {
            let item = tokio::select! {
                item = stream.next() => item,
                _ = shared.regen.notified() => {
                    if shared.generation.load(Ordering::Acquire) != generation { None } else { continue }
                }
            };
            let Some(item) = item else { break };
            let frame = match item {
                Ok(frame) => frame,
                Err(status) => {
                    shared.tally().error("stream", &status);
                    break;
                }
            };
            if shared.generation.load(Ordering::Acquire) != generation {
                break;
            }
            let tally = shared.tally();
            tally.frames.fetch_add(1, Ordering::Relaxed);
            tally.bytes.fetch_add(frame.len() as u64, Ordering::Relaxed);
            if let Some(sent) = stamp(&frame, b"lg:") {
                tally.delivered.fetch_add(1, Ordering::Relaxed);
                tally.lag.record(Duration::from_micros(now_us().saturating_sub(sent)));
            } else if let Some(sent) = stamp(&frame, b"tp:") {
                tally.topics.fetch_add(1, Ordering::Relaxed);
                tally.topic_lag.record(Duration::from_micros(now_us().saturating_sub(sent)));
            }
            for (field, value) in fields(&frame) {
                match (field, value) {
                    // event: remember its sequence
                    (1, Err(event)) => {
                        for (f, v) in fields(event) {
                            if let (3, Ok(seq)) = (f, v)
                                && seq as i64 > 0
                            {
                                last = Some(seq as i64);
                            }
                        }
                    }
                    // ready: the heads
                    (2, Err(ready)) => {
                        for (_, head) in fields(ready) {
                            if let Err(head) = head {
                                for (f, v) in fields(head) {
                                    if let (2, Ok(seq)) = (f, v) {
                                        last = Some(seq as i64);
                                    }
                                }
                            }
                        }
                        if !live {
                            live = true;
                            shared.ready.record(started.elapsed());
                            shared.live.fetch_add(1, Ordering::AcqRel);
                            permit.take();
                            let _ = ready_tx.send(());
                        }
                    }
                    _ => {}
                }
            }
        }
        drop(permit);
        if live {
            shared.live.fetch_sub(1, Ordering::AcqRel);
        }
        if shared.generation.load(Ordering::Acquire) == generation {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

/// Holds a plain stream open, counting what comes, until the process ends.
async fn hold(
    channel: Channel,
    token: String,
    path: &'static str,
    kind: Kind,
    shared: Arc<Shared>,
    storm: Arc<Semaphore>,
    ready_tx: tokio::sync::mpsc::UnboundedSender<()>,
) {
    loop {
        let permit = storm.acquire().await.unwrap();
        let opened = match kind {
            Kind::Dms => open_raw(&channel, &token, path, pb::WatchRequest {}).await,
            Kind::Friends => open_raw(&channel, &token, path, pb::WatchFriendsRequest {}).await,
            Kind::Presence => open_raw(&channel, &token, path, pb::WatchPresenceRequest {}).await,
        };
        let stream = match opened {
            Ok(s) => s,
            Err(status) => {
                drop(permit);
                shared.tally().error(kind.name(), &status);
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let mut permit = Some(permit);
        tokio::pin!(stream);
        while let Some(item) = stream.next().await {
            let frame = match item {
                Ok(frame) => frame,
                Err(status) => {
                    shared.tally().error(kind.name(), &status);
                    break;
                }
            };
            let tally = shared.tally();
            tally.frames.fetch_add(1, Ordering::Relaxed);
            tally.bytes.fetch_add(frame.len() as u64, Ordering::Relaxed);
            for (field, value) in fields(&frame) {
                let ready =
                    matches!((kind, field, &value), (Kind::Presence, 2, Ok(1)) | (Kind::Dms | Kind::Friends, 1, Ok(1)));
                if matches!((kind, field, &value), (Kind::Presence, 1, Err(_))) {
                    tally.presences.fetch_add(1, Ordering::Relaxed);
                } else if ready && permit.take().is_some() {
                    let _ = ready_tx.send(());
                }
            }
        }
        drop(permit);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum Kind {
    Dms,
    Friends,
    Presence,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Dms => "dms",
            Kind::Friends => "friends",
            Kind::Presence => "presence",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Kind::Dms => "/fuwa.v1.DirectMessageService/Watch",
            Kind::Friends => "/fuwa.v1.FriendService/WatchFriends",
            Kind::Presence => "/fuwa.v1.PresenceService/WatchPresence",
        }
    }
}

async fn wait_for(rx: &mut tokio::sync::mpsc::UnboundedReceiver<()>, n: usize, limit: Duration) -> usize {
    let deadline = Instant::now() + limit;
    let mut got = 0;
    while got < n {
        match tokio::time::timeout_at(deadline.into(), rx.recv()).await {
            Ok(Some(())) => got += 1,
            _ => break,
        }
    }
    got
}

/// Waits until the tally stops moving for `quiet`, or `limit` passes.
async fn settle(count: impl Fn() -> u64, quiet: Duration, limit: Duration) -> Duration {
    let started = Instant::now();
    let mut last = (count(), Instant::now());
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let frames = count();
        if frames != last.0 {
            last = (frames, Instant::now());
        } else if last.1.elapsed() >= quiet {
            return last.1.duration_since(started);
        }
        if started.elapsed() >= limit {
            return limit;
        }
    }
}

// ---------------------------------------------------------------- main

struct Report {
    out: Option<std::fs::File>,
}

impl Report {
    fn line(&mut self, value: serde_json::Value) {
        println!("{value}");
        if let Some(out) = &mut self.out {
            use std::io::Write;
            writeln!(out, "{value}").unwrap();
        }
    }
}

#[tokio::main]
async fn main() {
    let o = Options::parse();
    let dir = match &o.data {
        Some(d) => d.clone(),
        None => std::env::temp_dir().join(format!("fuwa-crowd-{}", std::process::id())),
    };
    std::fs::create_dir_all(&dir).unwrap();
    let saved = dir.join(format!("setup-{}.json", o.mode));
    let have: Option<Setup> = std::fs::read(&saved).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let instance = start(&o, &dir).await;
    let setup = setup(&instance.url, have, o.people).await;
    std::fs::write(&saved, serde_json::to_vec(&setup).unwrap()).unwrap();
    let people = o.people;
    let tokens: Vec<String> = setup.tokens[..people].to_vec();
    let mut report = Report { out: o.out.as_ref().map(|p| std::fs::File::create(p).unwrap()) };

    let shared = Arc::new(Shared {
        tally: Mutex::new(Arc::new(Tally::new())),
        generation: AtomicU64::new(0),
        regen: Notify::new(),
        live: AtomicUsize::new(0),
        ready: Hist::new(),
    });
    // Each kind of stream on connections of its own, `per_conn` streams each.
    let per_kind = people.div_ceil(o.per_conn);
    let conns: Vec<Channel> = (0..per_kind * 5).map(|_| connect(&instance.url)).collect();
    let conn_for = |kind: usize, i: usize| conns[kind * per_kind + i / o.per_conn].clone();
    let conn_of = |i: usize| conn_for(0, i);
    let storm = Arc::new(Semaphore::new(o.storm));
    // Let the instance settle after start (it reads every server's file).
    tokio::time::sleep(Duration::from_secs(3)).await;
    let base = instance.sample();
    report.line(serde_json::json!({"phase": "start", "people": people, "mode": o.mode, "per_conn": o.per_conn,
        "rss_mb": rss_mb(&base)}));

    // connect: everyone's events stream.
    let (ready_tx, mut ready_rx) = tokio::sync::mpsc::unbounded_channel();
    let before = instance.sample();
    let t = shared.fresh();
    let started = Instant::now();
    for (i, token) in tokens.iter().enumerate() {
        tokio::spawn(follow(
            conn_of(i),
            token.clone(),
            setup.server_id.clone(),
            shared.clone(),
            storm.clone(),
            ready_tx.clone(),
        ));
    }
    let got = wait_for(&mut ready_rx, people, Duration::from_secs(600)).await;
    let took = started.elapsed();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let after = instance.sample();
    let per_stream_kb =
        (instance.rss_total_kb() as f64 - before.rss_kb.iter().map(|(_, k)| *k).sum::<u64>() as f64) / people as f64;
    report.line(serde_json::json!({"phase": "connect", "live": got, "secs": took.as_secs_f64(),
        "ready_p50_ms": shared.ready.quantile(0.5), "ready_p99_ms": shared.ready.quantile(0.99),
        "kb_per_stream": per_stream_kb.round(), "rss_mb": rss_mb(&after), "cpu_pct": cpu_between(&before, &after),
        "errors": t.errors()}));

    // tab: what else a web tab holds open.
    if o.has("tab") {
        for kind in [Kind::Dms, Kind::Friends, Kind::Presence] {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let before = instance.sample();
            let t = shared.fresh();
            let started = Instant::now();
            for (i, token) in tokens.iter().enumerate() {
                tokio::spawn(hold(
                    conn_for(kind as usize + 1, i),
                    token.clone(),
                    kind.path(),
                    kind,
                    shared.clone(),
                    storm.clone(),
                    tx.clone(),
                ));
            }
            let got = wait_for(&mut rx, people, Duration::from_secs(600)).await;
            let took = started.elapsed();
            tokio::time::sleep(Duration::from_secs(5)).await;
            let after = instance.sample();
            let kb = (instance.rss_total_kb() as f64 - before.rss_kb.iter().map(|(_, k)| *k).sum::<u64>() as f64)
                / people as f64;
            report.line(serde_json::json!({"phase": format!("tab.{}", kind.name()), "live": got,
                "secs": took.as_secs_f64(), "kb_per_stream": kb.round(), "rss_mb": rss_mb(&after),
                "cpu_pct": cpu_between(&before, &after), "frames": t.frames.load(Ordering::Relaxed),
                "errors": t.errors()}));
        }
    }

    // online: everyone says they're here.
    if o.has("online") {
        let before = instance.sample();
        let t = shared.fresh();
        let started = Instant::now();
        let limit = Arc::new(Semaphore::new(o.storm));
        let done = Arc::new(AtomicUsize::new(0));
        let slowest = Arc::new(AtomicI64::new(0));
        for (i, token) in tokens.iter().enumerate() {
            let (channel, token, limit, done, slowest, shared) =
                (conn_for(4, i), token.clone(), limit.clone(), done.clone(), slowest.clone(), shared.clone());
            tokio::spawn(async move {
                let _permit = limit.acquire().await.unwrap();
                let begun = Instant::now();
                let mut client = pb::presence_service_client::PresenceServiceClient::new(channel);
                let req = pb::UpdatePresenceRequest { app: "web".into(), ..Default::default() };
                match client.update_presence(authed(&token, req)).await {
                    Ok(_) => {
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(s) => shared.tally().error("update_presence", &s),
                }
                slowest.fetch_max(begun.elapsed().as_millis() as i64, Ordering::Relaxed);
            });
        }
        let calls = loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let d = done.load(Ordering::Relaxed);
            if d + t.errors().values().sum::<u64>() as usize >= people || started.elapsed() > Duration::from_secs(900) {
                break started.elapsed();
            }
        };
        let quiet =
            settle(|| t.presences.load(Ordering::Relaxed), Duration::from_secs(5), Duration::from_secs(900)).await;
        let after = instance.sample();
        report.line(serde_json::json!({"phase": "online", "updates": done.load(Ordering::Relaxed),
            "calls_secs": calls.as_secs_f64(), "slowest_call_ms": slowest.load(Ordering::Relaxed),
            "delivered_secs": quiet.as_secs_f64(), "presences_sent": t.presences.load(Ordering::Relaxed),
            "per_person": t.presences.load(Ordering::Relaxed) as f64 / people as f64,
            "bytes_mb": t.bytes.load(Ordering::Relaxed) / 1_000_000, "rss_mb": rss_mb(&after),
            "cpu_pct": cpu_between(&before, &after), "errors": t.errors()}));
    }

    // idle: heartbeats only.
    if o.has("idle") {
        let before = instance.sample();
        let t = shared.fresh();
        tokio::time::sleep(Duration::from_secs(o.idle_secs)).await;
        let after = instance.sample();
        report
            .line(serde_json::json!({"phase": "idle", "secs": o.idle_secs, "frames": t.frames.load(Ordering::Relaxed),
            "rss_mb": rss_mb(&after), "cpu_pct": cpu_between(&before, &after), "errors": t.errors()}));
    }

    let owner = connect(&instance.url);
    // fanout: one channel, everyone reading.
    if o.has("fanout") {
        for &rate in &o.rates {
            let before = instance.sample();
            let t = shared.fresh();
            let stop = Arc::new(AtomicBool::new(false));
            let sent = Arc::new(AtomicU64::new(0));
            let send_lag = Arc::new(Hist::new());
            let mut senders = vec![];
            let workers = (rate.ceil() as usize).clamp(1, 16);
            let every = Duration::from_secs_f64(workers as f64 / rate);
            for k in 0..workers {
                let (owner, token, setup, stop, sent, send_lag) =
                    (owner.clone(), setup.owner.clone(), setup.clone(), stop.clone(), sent.clone(), send_lag.clone());
                senders.push(tokio::spawn(async move {
                    tokio::time::sleep(every.mul_f64(k as f64 / workers as f64)).await;
                    let mut tick = tokio::time::interval(every);
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    let mut messages = pb::message_service_client::MessageServiceClient::new(owner);
                    while !stop.load(Ordering::Relaxed) {
                        tick.tick().await;
                        let content =
                            format!("lg:{} hello from the crowd test, a line about the length people type", now_us());
                        let req = pb::SendMessageRequest {
                            server_id: setup.server_id.clone(),
                            channel_id: setup.text.clone(),
                            content,
                            ..Default::default()
                        };
                        let begun = Instant::now();
                        if messages.send_message(authed(&token, req)).await.is_ok() {
                            sent.fetch_add(1, Ordering::Relaxed);
                            send_lag.record(begun.elapsed());
                        }
                    }
                }));
            }
            tokio::time::sleep(Duration::from_secs(o.step_secs)).await;
            stop.store(true, Ordering::Relaxed);
            for s in senders {
                let _ = s.await;
            }
            let sending = before.at.elapsed();
            let drained =
                settle(|| t.delivered.load(Ordering::Relaxed), Duration::from_secs(3), Duration::from_secs(120)).await;
            let after = instance.sample();
            let expected = sent.load(Ordering::Relaxed) * people as u64;
            let delivered = t.delivered.load(Ordering::Relaxed);
            report.line(serde_json::json!({"phase": "fanout", "rate": rate, "sent": sent.load(Ordering::Relaxed),
                "send_p99_ms": send_lag.quantile(0.99),
                "delivered": delivered, "expected": expected,
                "deliveries_per_sec": (delivered as f64 / sending.as_secs_f64()).round(),
                "lag_p50_ms": t.lag.quantile(0.5), "lag_p99_ms": t.lag.quantile(0.99),
                "drain_secs": drained.as_secs_f64(), "live": shared.live.load(Ordering::Relaxed),
                "rss_mb": rss_mb(&after), "cpu_pct": cpu_between(&before, &after), "errors": t.errors()}));
            if t.lag.quantile(0.99) > 10_000.0 || delivered < expected * 9 / 10 {
                break;
            }
        }
    }

    // access: a topic change, which makes every stream work out what it can see again.
    if o.has("access") {
        let before = instance.sample();
        let t = shared.fresh();
        let mut channels = pb::channel_service_client::ChannelServiceClient::new(owner.clone());
        let changes = 3;
        for _ in 0..changes {
            let req = pb::UpdateChannelRequest {
                server_id: setup.server_id.clone(),
                channel_id: setup.quiet.clone(),
                topic: Some(format!("tp:{} what this channel is for", now_us())),
                ..Default::default()
            };
            channels.update_channel(authed(&setup.owner, req)).await.unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let drained =
            settle(|| t.topics.load(Ordering::Relaxed), Duration::from_secs(3), Duration::from_secs(300)).await;
        let after = instance.sample();
        report.line(serde_json::json!({"phase": "access", "changes": changes, "got": t.topics.load(Ordering::Relaxed),
            "expected": changes * people, "lag_p50_ms": t.topic_lag.quantile(0.5), "lag_p99_ms": t.topic_lag.quantile(0.99),
            "drain_secs": drained.as_secs_f64(), "rss_mb": rss_mb(&after), "cpu_pct": cpu_between(&before, &after),
            "errors": t.errors()}));
    }

    // members: the member list the web app loads for every server.
    if o.has("members") {
        let before = instance.sample();
        let servers = pb::server_service_client::ServerServiceClient::new(owner.clone());
        let begun = Instant::now();
        let req = pb::ListMembersRequest { server_id: setup.server_id.clone() };
        let one =
            servers.clone().max_decoding_message_size(512 * 1024 * 1024).list_members(authed(&setup.owner, req)).await;
        let one_ms = begun.elapsed().as_millis();
        let (count, bytes) = match &one {
            Ok(r) => (r.get_ref().members.len(), prost::Message::encoded_len(r.get_ref())),
            Err(s) => {
                eprintln!("list members: {s}");
                (0, 0)
            }
        };
        let begun = Instant::now();
        let mut many = vec![];
        for i in 0..50 {
            let mut servers = pb::server_service_client::ServerServiceClient::new(conn_for(4, i * people / 50))
                .max_decoding_message_size(512 * 1024 * 1024);
            let req = pb::ListMembersRequest { server_id: setup.server_id.clone() };
            let token = tokens[i * people / 50].clone();
            many.push(tokio::spawn(async move { servers.list_members(authed(&token, req)).await.is_ok() }));
        }
        let mut ok = 0;
        for m in many {
            ok += m.await.unwrap() as usize;
        }
        let many_ms = begun.elapsed().as_millis();
        let after = instance.sample();
        report.line(serde_json::json!({"phase": "members", "members": count, "bytes": bytes, "one_ms": one_ms,
            "fifty_ok": ok, "fifty_ms": many_ms, "rss_mb": rss_mb(&after), "cpu_pct": cpu_between(&before, &after)}));
    }

    // reconnect: everyone drops and follows again, as after a restart.
    if o.has("reconnect") {
        let before = instance.sample();
        let t = shared.fresh();
        while ready_rx.try_recv().is_ok() {}
        let started = Instant::now();
        shared.generation.fetch_add(1, Ordering::AcqRel);
        shared.regen.notify_waiters();
        let got = wait_for(&mut ready_rx, people, Duration::from_secs(600)).await;
        let took = started.elapsed();
        let after = instance.sample();
        report.line(serde_json::json!({"phase": "reconnect", "live": got, "secs": took.as_secs_f64(),
            "rss_mb": rss_mb(&after), "cpu_pct": cpu_between(&before, &after), "errors": t.errors()}));
    }

    instance.stop();
    std::process::exit(0);
}
