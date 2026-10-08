//! Your camera and your shared screen, going out (the web's `openCamera`,
//! `openScreen` and the encodings in calls/video.ts): pictures taken on a
//! thread of their own, then encoded in three sizes at once (simulcast "l",
//! "m" and "h") on another, at the web's sizes and bitrates. Only the newest
//! picture waits for the encoder, so a slow moment drops pictures rather
//! than delaying them. Your own preview comes from the same pictures.
//!
//! Cameras come through nokhwa (V4L2, AVFoundation, Media Foundation),
//! screens and windows through scap (X11, ScreenCaptureKit, Windows
//! Graphics Capture). With `FUWA_DESKTOP_FAKE_VIDEO=1`, a moving test
//! pattern stands in for both, for machines without either.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use tokio::sync::mpsc;

use super::access::{self, Access, Device};
use super::video::Videos;
use super::vp8::{self, Encoder, Layout, Picture, Size, Yuv};

/// Set, a test pattern stands in for the camera and the screen.
pub const FAKE_VIDEO: &str = "FUWA_DESKTOP_FAKE_VIDEO";

/// What to send from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A camera by name, "" for the first one.
    Camera(String),
    /// A screen or window to share (`Screen::id`), or the main screen.
    Screen(Option<u32>),
    /// A moving test pattern instead of a camera or screen; `seed` tints it.
    Pattern { screen: bool, seed: u32 },
}

/// Why the camera or screen stopped, or never started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The system won't let the app use it (macOS's privacy settings).
    Blocked,
    NotFound,
    Busy,
    Failed,
}

/// A frame out of the encoder, ready to go: whose track and which size.
#[derive(Debug)]
pub struct Filmed {
    pub screen: bool,
    pub rid: &'static str,
    pub frame: Vec<u8>,
    pub keyframe: bool,
    pub taken: Instant,
}

/// The newest picture taken, waiting for the encoder, and a buffer the
/// encoder is done with, for the next picture.
#[derive(Default)]
struct Latest {
    picture: Mutex<Option<(Yuv, Instant)>>,
    spare: Mutex<Option<Yuv>>,
    ready: Condvar,
}

impl Latest {
    /// Hands over a picture, getting back a buffer to write the next into:
    /// the one the encoder never took, or one it's done with.
    fn put(&self, yuv: Yuv, taken: Instant) -> Yuv {
        let old = self.picture.lock().replace((yuv, taken));
        self.ready.notify_one();
        old.map(|(y, _)| y).or_else(|| self.spare.lock().take()).unwrap_or_default()
    }

    fn give_back(&self, yuv: Yuv) {
        *self.spare.lock() = Some(yuv);
    }

    fn take(&self, wait: Duration) -> Option<(Yuv, Instant)> {
        let mut picture = self.picture.lock();
        if picture.is_none() {
            self.ready.wait_for(&mut picture, wait);
        }
        picture.take()
    }
}

/// A camera or screen going out while it's on. Dropping it stops it.
pub struct Sending {
    pub screen: bool,
    stop: Arc<AtomicBool>,
    /// Sizes asked for a keyframe, by [`vp8::CAMERA`]'s order.
    keyframes: Arc<[AtomicBool; 3]>,
    videos: Arc<Videos>,
    feed: String,
}

impl Drop for Sending {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.videos.forget(&self.feed);
    }
}

impl Sending {
    /// Starts taking pictures from `source` and encoding them into `out`.
    /// `feed` is your own (for the preview), `mirror` whether it shows
    /// mirrored to you, and `failed` hears it if the source stops.
    pub fn start(
        source: Source,
        feed: String,
        videos: Arc<Videos>,
        mirror: Arc<AtomicBool>,
        out: Option<mpsc::Sender<Filmed>>,
        failed: impl Fn(Failure) + Send + 'static,
    ) -> Self {
        let screen = matches!(source, Source::Screen(_) | Source::Pattern { screen: true, .. });
        let stop = Arc::new(AtomicBool::new(false));
        let keyframes = Arc::new([AtomicBool::new(true), AtomicBool::new(true), AtomicBool::new(true)]);
        let latest = Arc::new(Latest::default());
        {
            let (stop, latest) = (stop.clone(), latest.clone());
            let _ = std::thread::Builder::new().name("fuwa-video-take".into()).spawn(move || {
                let result = match source {
                    Source::Camera(name) => take_camera(&name, &stop, &latest),
                    Source::Screen(id) => take_screen(id, &stop, &latest),
                    Source::Pattern { screen, seed } => take_pattern(screen, seed, &stop, &latest),
                };
                if let Err(failure) = result
                    && !stop.load(Ordering::Relaxed)
                {
                    failed(failure);
                }
            });
        }
        {
            let (stop, keyframes, videos, feed) = (stop.clone(), keyframes.clone(), videos.clone(), feed.clone());
            let _ = std::thread::Builder::new().name("fuwa-video-out".into()).spawn(move || {
                encode(screen, &stop, &latest, &keyframes, &videos, &feed, &mirror, out.as_ref());
            });
        }
        Self { screen, stop, keyframes, videos, feed }
    }

    /// The media part asked for a keyframe: of one size, or (None) of all.
    pub fn keyframe(&self, rid: Option<&str>) {
        for (n, size) in vp8::CAMERA.iter().enumerate() {
            if rid.is_none_or(|r| r == size.rid) {
                self.keyframes[n].store(true, Ordering::Relaxed);
            }
        }
    }
}

/// The encoder's thread: each picture in up to three sizes, each size at
/// its own frame rate, and the preview.
#[allow(clippy::too_many_arguments)]
fn encode(
    screen: bool,
    stop: &AtomicBool,
    latest: &Latest,
    keyframes: &[AtomicBool; 3],
    videos: &Videos,
    feed: &str,
    mirror: &AtomicBool,
    out: Option<&mpsc::Sender<Filmed>>,
) {
    let sizes: [Size; 3] = if screen { vp8::SCREEN } else { vp8::CAMERA };
    let mut encoders: [Option<Encoder>; 3] = [None, None, None];
    let mut last: [Option<Instant>; 3] = [None; 3];
    let (mut half, mut quarter) = (Yuv::default(), Yuv::default());
    let mut preview = Picture::default();
    while !stop.load(Ordering::Relaxed) {
        let Some((full, taken)) = latest.take(Duration::from_millis(200)) else { continue };
        vp8::half(&full, &mut half);
        vp8::half(&half, &mut quarter);
        let pictures = [&quarter, &half, &full];
        // Only a preview (the camera check in settings): nothing to encode.
        let Some(out) = out else {
            preview_of(
                &[&quarter, &half, &full],
                videos,
                feed,
                !screen && mirror.load(Ordering::Relaxed),
                &mut preview,
            );
            latest.give_back(full);
            continue;
        };
        for (n, size) in sizes.iter().enumerate() {
            let yuv = pictures[n];
            // A size's own frame rate, give or take a few milliseconds.
            let every = Duration::from_micros(1_000_000 / u64::from(size.fps)).saturating_sub(Duration::from_millis(5));
            if last[n].is_some_and(|at| taken.saturating_duration_since(at) < every) {
                continue;
            }
            last[n] = Some(taken);
            if encoders[n].as_ref().is_none_or(|e| (e.width, e.height) != (yuv.width, yuv.height)) {
                encoders[n] = Encoder::new(yuv.width, yuv.height, size.bitrate, size.fps, screen).ok();
                keyframes[n].store(true, Ordering::Relaxed);
            }
            let Some(encoder) = encoders[n].as_mut() else { continue };
            let key = keyframes[n].swap(false, Ordering::Relaxed);
            for encoded in encoder.encode(yuv, key) {
                let filmed = Filmed { screen, rid: size.rid, frame: encoded.data, keyframe: encoded.keyframe, taken };
                // No room: this size waits for a keyframe, rather than breaking up.
                if out.try_send(filmed).is_err() {
                    keyframes[n].store(true, Ordering::Relaxed);
                }
            }
        }
        preview_of(&[&quarter, &half, &full], videos, feed, !screen && mirror.load(Ordering::Relaxed), &mut preview);
        latest.give_back(full);
    }
}

/// Your own picture, at the size you show it (none when it isn't shown).
fn preview_of(sizes: &[&Yuv; 3], videos: &Videos, feed: &str, mirror: bool, preview: &mut Picture) {
    let yuv = match super::video::layer_for(videos.wanted(feed)) {
        "l" => sizes[0],
        "m" => sizes[1],
        "h" => sizes[2],
        _ => return,
    };
    vp8::yuv_to_bgra(yuv, mirror, &mut preview.bgra);
    preview.width = yuv.width;
    preview.height = yuv.height;
    videos.publish(feed, preview);
}

// ───────────────────────── Cameras ─────────────────────────

/// The cameras this computer has, by name.
pub fn cameras() -> Vec<String> {
    if std::env::var_os(FAKE_VIDEO).is_some() {
        return vec!["Test pattern".into()];
    }
    nokhwa::query(nokhwa::utils::ApiBackend::Auto)
        .map(|list| list.into_iter().map(|c| c.human_name()).collect())
        .unwrap_or_default()
}

/// Waits for the system's answer about the camera (macOS asks once).
fn camera_access(stop: &AtomicBool) -> Result<(), Failure> {
    loop {
        match access::check(Device::Camera) {
            Access::Allowed => return Ok(()),
            Access::Blocked => return Err(Failure::Blocked),
            Access::Asking if stop.load(Ordering::Relaxed) => return Err(Failure::Failed),
            Access::Asking => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

fn take_camera(name: &str, stop: &AtomicBool, latest: &Latest) -> Result<(), Failure> {
    use nokhwa::utils::{ApiBackend, CameraFormat, FrameFormat, RequestedFormat, RequestedFormatType, Resolution};
    camera_access(stop)?;
    #[cfg(target_os = "macos")]
    nokhwa::nokhwa_initialize(|_| {});
    let list = nokhwa::query(ApiBackend::Auto).map_err(|_| Failure::NotFound)?;
    let info = list.iter().find(|c| c.human_name() == name).or_else(|| list.first()).ok_or(Failure::NotFound)?;
    const FORMATS: [FrameFormat; 6] = [
        FrameFormat::MJPEG,
        FrameFormat::YUYV,
        FrameFormat::NV12,
        FrameFormat::RAWRGB,
        FrameFormat::RAWBGR,
        FrameFormat::GRAY,
    ];
    // nokhwa's Closest only looks at the one frame format it's given, so
    // each is asked for in turn: MJPEG where the camera has it (most USB
    // ones), else what it sends raw (YUYV for every Mac camera). Any format
    // at all comes last, for cameras without 1280x720.
    let asks = FORMATS
        .map(|f| RequestedFormatType::Closest(CameraFormat::new(Resolution::new(1280, 720), f, 30)))
        .into_iter()
        .chain([RequestedFormatType::None]);
    let mut camera = None;
    let mut last = None;
    for ask in asks {
        match nokhwa::Camera::new(info.index().clone(), RequestedFormat::with_formats(ask, &FORMATS)) {
            Ok(opened) => {
                camera = Some(opened);
                break;
            }
            Err(why) if in_use(&why) => return Err(Failure::Busy),
            Err(why) => last = Some(why),
        }
    }
    let Some(mut camera) = camera else {
        tracing::warn!(error = ?last, "no camera format would open");
        return Err(Failure::Failed);
    };
    camera.open_stream().map_err(|why| {
        tracing::warn!(error = %why, "camera wouldn't start");
        if in_use(&why) { Failure::Busy } else { Failure::Failed }
    })?;
    let mut yuv = Yuv::default();
    let mut rgb = Vec::new();
    let mut misses = 0;
    while !stop.load(Ordering::Relaxed) {
        let Ok(buffer) = camera.frame() else {
            misses += 1;
            if misses > 30 {
                return Err(Failure::Failed);
            }
            continue;
        };
        misses = 0;
        let taken = Instant::now();
        let (w, h) = (buffer.resolution().width(), buffer.resolution().height());
        let bytes = buffer.buffer();
        let (fw, fh) = vp8::fit(w, h, vp8::CAMERA_MOST.0, vp8::CAMERA_MOST.1);
        yuv.resize(fw, fh);
        match buffer.source_frame_format() {
            FrameFormat::MJPEG => {
                let Some((jw, jh)) = decode_jpeg(bytes, &mut rgb) else { continue };
                let (fw, fh) = vp8::fit(jw, jh, vp8::CAMERA_MOST.0, vp8::CAMERA_MOST.1);
                yuv.resize(fw, fh);
                vp8::from_rgb(&rgb, jw as usize * 3, jw, jh, Layout::RGB, &mut yuv);
            }
            FrameFormat::YUYV => vp8::from_yuyv(bytes, w, h, &mut yuv),
            FrameFormat::NV12 => vp8::from_nv12(bytes, w, h, &mut yuv),
            FrameFormat::RAWRGB => vp8::from_rgb(bytes, w as usize * 3, w, h, Layout::RGB, &mut yuv),
            FrameFormat::RAWBGR => vp8::from_rgb(bytes, w as usize * 3, w, h, Layout::BGR, &mut yuv),
            FrameFormat::GRAY => {
                let (ys, us, vs) = yuv.planes_mut();
                let (fw, fh) = (fw as usize, fh as usize);
                for (n, y) in ys.iter_mut().enumerate() {
                    let (x, row) = (n % fw * w as usize / fw, n / fw * h as usize / fh);
                    *y = bytes.get(row * w as usize + x).copied().unwrap_or(0);
                }
                us.fill(128);
                vs.fill(128);
            }
        }
        yuv = latest.put(yuv, taken);
    }
    let _ = camera.stop_stream();
    Ok(())
}

/// Whether the camera wouldn't open because another app has it, which the
/// backends say only in words ("Already in use" on macOS, EBUSY's "busy"
/// on Linux).
fn in_use(why: &nokhwa::NokhwaError) -> bool {
    let text = why.to_string().to_lowercase();
    text.contains("in use") || text.contains("busy")
}

/// An MJPEG frame as packed RGB in `out`, giving its size.
fn decode_jpeg(bytes: &[u8], out: &mut Vec<u8>) -> Option<(u32, u32)> {
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder.decode_headers().ok()?;
    let (w, h) = decoder.dimensions()?;
    out.resize(decoder.output_buffer_size()?, 0);
    decoder.decode_into(out).ok()?;
    Some((w as u32, h as u32))
}

// ───────────────────────── Screens ─────────────────────────

/// A screen or window that can be shared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub id: u32,
    pub name: String,
    /// A whole screen, not one window.
    pub display: bool,
}

/// What can be shared: screens first, then windows.
pub fn screens() -> Vec<Screen> {
    if std::env::var_os(FAKE_VIDEO).is_some() {
        return vec![Screen { id: 0, name: "Test pattern".into(), display: true }];
    }
    if !zed_scap::is_supported() {
        return Vec::new();
    }
    let mut out: Vec<Screen> = zed_scap::get_all_targets()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|target| match target {
            zed_scap::Target::Display(d) => Some(Screen { id: d.id, name: d.title, display: true }),
            zed_scap::Target::Window(w) if !w.title.trim().is_empty() => {
                Some(Screen { id: w.id, name: w.title, display: false })
            }
            zed_scap::Target::Window(_) => None,
        })
        .collect();
    out.sort_by_key(|s| !s.display);
    out
}

/// Whether the system lets the app see the screen: macOS asks the first
/// time, and after a no only System Settings changes it.
pub fn screen_access() -> Access {
    if zed_scap::has_permission() {
        return Access::Allowed;
    }
    if zed_scap::request_permission() { Access::Allowed } else { Access::Blocked }
}

/// Opens the system's settings for screen recording.
pub fn open_screen_settings() {
    if cfg!(target_os = "macos") {
        let _ = open::that_detached("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture");
    }
}

fn take_screen(id: Option<u32>, stop: &AtomicBool, latest: &Latest) -> Result<(), Failure> {
    use zed_scap::frame::Frame;
    if screen_access() != Access::Allowed {
        return Err(Failure::Blocked);
    }
    let target = id.and_then(|id| {
        zed_scap::get_all_targets().unwrap_or_default().into_iter().find(|t| match t {
            zed_scap::Target::Display(d) => d.id == id,
            zed_scap::Target::Window(w) => w.id == id,
        })
    });
    let options = zed_scap::capturer::Options {
        fps: 30,
        show_cursor: true,
        target,
        output_type: zed_scap::frame::FrameType::BGRAFrame,
        ..Default::default()
    };
    let mut capturer = zed_scap::capturer::Capturer::build(options).map_err(|_| Failure::Failed)?;
    capturer.start_capture();
    let mut yuv = Yuv::default();
    let result = loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        let Ok(frame) = capturer.get_next_frame() else { break Err(Failure::Failed) };
        let taken = Instant::now();
        let (w, h, data, layout) = match &frame {
            Frame::BGRA(f) => (f.width, f.height, &f.data, Layout::BGRA),
            Frame::BGRx(f) => (f.width, f.height, &f.data, Layout::BGRA),
            Frame::BGR0(f) => (f.width, f.height, &f.data, Layout::BGRA),
            Frame::RGBx(f) => (f.width, f.height, &f.data, Layout::RGBA),
            Frame::XBGR(f) => (f.width, f.height, &f.data, Layout::XBGR),
            Frame::RGB(f) => (f.width, f.height, &f.data, Layout::RGB),
            Frame::YUVFrame(_) => continue,
        };
        let (w, h) = (w.max(1) as u32, h.max(1) as u32);
        let stride = if h > 0 { data.len() / h as usize } else { 0 };
        if stride < w as usize * layout.bytes {
            continue;
        }
        let (fw, fh) = vp8::fit(w, h, vp8::SCREEN_MOST.0, vp8::SCREEN_MOST.1);
        yuv.resize(fw, fh);
        vp8::from_rgb(data, stride, w, h, layout, &mut yuv);
        yuv = latest.put(yuv, taken);
    };
    capturer.stop_capture();
    result
}

// ───────────────────────── The test pattern ─────────────────────────

/// A test pattern, for machines without a camera or a screen to share: a
/// tinted background, bars, and a square going round, 30 times a second.
fn take_pattern(screen: bool, seed: u32, stop: &AtomicBool, latest: &Latest) -> Result<(), Failure> {
    let (w, h) = if screen { (1280, 720) } else { (640, 360) };
    let mut yuv = Yuv::new(w, h);
    let started = Instant::now();
    let mut n = 0u64;
    while !stop.load(Ordering::Relaxed) {
        yuv.resize(w, h);
        draw_pattern(&mut yuv, seed, screen, n);
        yuv = latest.put(yuv, Instant::now());
        n += 1;
        let due = started + Duration::from_millis(n * 1000 / 30);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
    Ok(())
}

/// Draws frame `n` of the pattern.
pub fn draw_pattern(yuv: &mut Yuv, seed: u32, screen: bool, n: u64) {
    let (w, h) = (yuv.width as usize, yuv.height as usize);
    let (u0, v0) = (64 + (seed % 128) as u8, 64 + ((seed / 128) % 128) as u8);
    let t = n as f32 / 30.0;
    let (cx, cy) = ((0.5 + 0.35 * t.cos()) * w as f32, (0.5 + 0.3 * t.sin()) * h as f32);
    let side = h as f32 / 5.0;
    let (ys, us, vs) = yuv.planes_mut();
    for y in 0..h {
        for x in 0..w {
            let in_square = (x as f32 - cx).abs() < side / 2.0 && (y as f32 - cy).abs() < side / 2.0;
            let bar = (x * 8 / w) as u8;
            ys[y * w + x] = if in_square {
                235
            } else if screen && (x % 160 < 4 || y % 120 < 4) {
                40
            } else if y > h * 4 / 5 {
                16 + bar * 28
            } else {
                110
            };
        }
    }
    let cw = w.div_ceil(2);
    for y in 0..h.div_ceil(2) {
        for x in 0..cw {
            let bottom = y * 2 > h * 4 / 5;
            us[y * cw + x] = if bottom { 128 } else { u0 };
            vs[y * cw + x] = if bottom { 128 } else { v0 };
        }
    }
}
