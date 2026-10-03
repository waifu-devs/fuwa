//! How fast the window draws, for working on it: with `FUWA_DESKTOP_PERF=1`
//! the app logs when its first frame was ready, then every few seconds how
//! many frames it drew, how long building them took (render, layout and
//! paint, on the CPU) and how much memory it holds. Off, it costs a check
//! of a flag per frame. At debug level it logs every frame's time too.
//!
//! The anonymous reports (`core::reports`) take two things from here,
//! whatever the variable says: how long the first frame took to come after
//! start (`startup`) and frames that took over 50 ms to build (`frame.long`).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{AnyElement, IntoElement as _, ParentElement as _, Styled as _, canvas, div};
use parking_lot::Mutex;

use crate::core::reports;

const EVERY: Duration = Duration::from_secs(5);

struct State {
    started: Instant,
    first: bool,
    window: Instant,
    frames: Vec<f64>,
}

fn state() -> Option<&'static Mutex<State>> {
    static STATE: OnceLock<Option<Mutex<State>>> = OnceLock::new();
    STATE
        .get_or_init(|| {
            std::env::var_os("FUWA_DESKTOP_PERF").map(|_| {
                let now = Instant::now();
                Mutex::new(State { started: now, first: true, window: now, frames: Vec::new() })
            })
        })
        .as_ref()
}

/// When the app started, for the reports' `startup`.
static STARTED: OnceLock<Instant> = OnceLock::new();
/// Whether the first frame has been drawn yet.
static FIRST_FRAME: AtomicBool = AtomicBool::new(true);

/// Called as the app starts, so startup counts from here.
pub fn start() {
    STARTED.get_or_init(Instant::now);
    let _ = state();
}

/// Notes how long after start a step of starting up finished.
pub fn mark(what: &str) {
    if let Some(state) = state() {
        let ms = state.lock().started.elapsed().as_secs_f64() * 1000.0;
        tracing::info!(target: "fuwa_desktop::perf", "{what} {ms:.0} ms after start");
    }
}

/// Times something done outside of drawing (reading the store after a
/// change), logged at debug level when it's dropped.
pub struct Timed(Option<(&'static str, Instant)>);

pub fn time(what: &'static str) -> Timed {
    Timed(state().map(|_| (what, Instant::now())))
}

impl Drop for Timed {
    fn drop(&mut self) {
        if let Some((what, began)) = self.0 {
            tracing::debug!(target: "fuwa_desktop::perf", "{what} {:.3} ms", began.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

/// Called at the top of the window's render: when it began, if anything
/// measures frames.
pub fn frame() -> Option<Instant> {
    if state().is_none() && !reports::enabled() {
        // Startup is only counted from the first frame itself.
        FIRST_FRAME.store(false, Ordering::Relaxed);
        return None;
    }
    Some(Instant::now())
}

/// Wraps the window's whole tree: an empty canvas painted after everything
/// else notes when the frame was done.
pub fn measured(root: AnyElement, began: Option<Instant>) -> AnyElement {
    let Some(began) = began else { return root };
    div()
        .size_full()
        .child(root)
        .child(canvas(|_, _, _| {}, move |_, _, _, _| done(began)).absolute().size_0())
        .into_any_element()
}

fn done(began: Instant) {
    let took = began.elapsed();
    if took > reports::LONG_FRAME {
        reports::timing("frame.long", took);
    }
    if FIRST_FRAME.swap(false, Ordering::Relaxed)
        && let Some(started) = STARTED.get()
    {
        reports::timing("startup", started.elapsed());
    }
    let Some(state) = state() else { return };
    let ms = took.as_secs_f64() * 1000.0;
    let mut s = state.lock();
    if s.first {
        s.first = false;
        tracing::info!(target: "fuwa_desktop::perf", "first frame {:.0} ms after start, {}", s.started.elapsed().as_secs_f64() * 1000.0, memory());
    }
    // Each frame, for tools that want them all (`RUST_LOG=fuwa_desktop::perf=debug`).
    tracing::debug!(target: "fuwa_desktop::perf", "frame {ms:.3} ms");
    s.frames.push(ms);
    if s.window.elapsed() >= EVERY {
        let secs = s.window.elapsed().as_secs_f64();
        let mut f = std::mem::take(&mut s.frames);
        f.sort_by(f64::total_cmp);
        let at = |q: f64| f[((f.len() - 1) as f64 * q).round() as usize];
        tracing::info!(
            target: "fuwa_desktop::perf",
            "{:.1} frames/s, build p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms, {}",
            f.len() as f64 / secs,
            at(0.5),
            at(0.95),
            at(1.0),
            memory()
        );
        s.window = Instant::now();
    }
}

/// What the process holds in memory, where the system says.
fn memory() -> String {
    #[cfg(target_os = "linux")]
    if let Some(kb) = std::fs::read_to_string("/proc/self/status").ok().and_then(|s| {
        s.lines()
            .find_map(|l| l.strip_prefix("VmRSS:").and_then(|v| v.trim().trim_end_matches(" kB").parse::<u64>().ok()))
    }) {
        return format!("{:.0} MB resident", kb as f64 / 1024.0);
    }
    "memory unknown".into()
}
