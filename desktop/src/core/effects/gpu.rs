//! Draws the backdrop's shaders with wgpu, offscreen, for the window to show
//! as pictures (the web's `lib/effects/gpu.ts`; GPUI has no way to run a
//! shader of our own). One thread owns a GPU device of its own; the window
//! asks for a frame of a layer each time it draws one, and takes the frame
//! back once it's made, a frame later.
//!
//! Costs are kept small: one full-screen pass at a fraction of the window's
//! size (effects are soft, so a frame is at most `FULL_SIDE` pixels on its
//! long side, aurora half that), read back and handed over as BGRA. Nothing
//! is drawn unless the window asks, which it does at most 30 times a second
//! while it's in front and once while it's still.
//!
//! Custom shaders get more care, since anyone can write one: they're checked
//! and compiled before they draw, their first frames are timed (at three
//! quarters, then half, then a third of the resolution until one fits), and
//! they're timed again every couple of seconds while they run. A frame that
//! doesn't come back, a lost GPU or one that refuses the shader is blamed on
//! it (`status.rs`), and the window shows the shader's fallback.
//!
//! Where there's no GPU adapter, or the GPU was lost, `gpu()` says so and the
//! window draws the built-in effects by hand (`ui/effects.rs`).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use parking_lot::{Condvar, Mutex};

use crate::core::effects::custom::{full_source, shader_id, shader_line, shader_problem};
use crate::core::effects::shaders;
use crate::core::effects::status::{self, ShaderStatus};
use crate::core::i18n::t;
use crate::core::themes::Effect;

/// Frames a second while an effect moves, like the web's.
pub const FPS: u32 = 30;
/// The long side of a frame at full resolution, in pixels. Effects are soft
/// and stretched to the window, so this keeps reading frames back cheap.
pub const FULL_SIDE: f32 = 640.0;
/// Resolutions a custom shader is tried at, until its frames fit the budget.
const CUSTOM_SCALES: [f32; 3] = [0.75, 0.5, 0.34];
/// A custom shader's frame may take this long on the GPU (a 30 fps frame is 33 ms, and the app draws too).
const BUDGET_MS: f32 = 20.0;
/// A frame that hasn't come back after this long means the shader is stuck.
const STUCK: Duration = Duration::from_millis(2500);
/// A layer the window hasn't asked for in this long is let go.
const FORGET: Duration = Duration::from_secs(3);
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// What a layer draws: a built-in effect, or the WGSL of a custom shader.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    Effect(Effect),
    Custom(String),
}

/// What a frame is drawn with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inputs {
    /// The theme's primary, the primary turned 48 degrees, its background and its text (sRGB, 0 to 1).
    pub colors: [[f32; 4]; 4],
    /// The effect's clock, in seconds.
    pub time: f32,
    /// The strength, 0 to 1.
    pub intensity: f32,
    /// Where the pointer is in the layer, 0 to 1 across and down, if it's known.
    pub pointer: Option<[f32; 2]>,
    /// Reduced motion or speed 0: the pointer is followed at once instead of eased.
    pub still: bool,
}

/// A finished frame: BGRA, straight alpha, `width` by `height`.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Whether effects can be drawn on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gpu {
    /// Nobody asked yet.
    Unknown,
    /// Looking for an adapter.
    Starting,
    Ready,
    /// No adapter, or the GPU was lost or stopped.
    Missing,
}

/// A compiler error, at the shader's own line where it's in the shader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

struct Layer {
    source: Source,
    /// The layer's size on screen, in pixels.
    size: (u32, u32),
    inputs: Inputs,
    dirty: bool,
    asked: Instant,
    frame: Option<Frame>,
}

type Check = (String, oneshot::Sender<Option<Vec<Diagnostic>>>);

struct Jobs {
    gpu: Gpu,
    /// Whether a lost GPU may be looked for again (after "Try it again").
    again: bool,
    layers: HashMap<u64, Layer>,
    checks: Vec<Check>,
    /// Custom shaders to compile afresh, by id.
    forget: Vec<String>,
}

static JOBS: LazyLock<Mutex<Jobs>> = LazyLock::new(|| {
    Mutex::new(Jobs { gpu: Gpu::Unknown, again: false, layers: HashMap::new(), checks: Vec::new(), forget: Vec::new() })
});
static WAKE: Condvar = Condvar::new();

/// Whether effects can be drawn on the GPU now.
pub fn gpu() -> Gpu {
    JOBS.lock().gpu
}

fn wake_up(jobs: &mut Jobs) {
    if jobs.gpu == Gpu::Unknown {
        jobs.gpu = Gpu::Starting;
        let spawned = std::thread::Builder::new().name("fuwa-effects".into()).spawn(|| {
            if std::panic::catch_unwind(run).is_err() {
                give_up(false);
            }
        });
        if spawned.is_err() {
            jobs.gpu = Gpu::Missing;
        }
    }
    WAKE.notify_one();
}

/// The id of a layer showing `source` at `size` pixels: layers alike share their frames.
pub fn layer_key(source: &Source, size: (u32, u32)) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut h);
    size.hash(&mut h);
    h.finish()
}

/// Asks for a frame of a layer `size` pixels big; it comes back through `take`.
/// Asking again with the same inputs draws nothing new.
pub fn request(source: &Source, size: (u32, u32), inputs: Inputs) -> u64 {
    let size = (size.0.clamp(1, 8192), size.1.clamp(1, 8192));
    let key = layer_key(source, size);
    let mut jobs = JOBS.lock();
    let now = Instant::now();
    let fresh = match jobs.layers.get_mut(&key) {
        Some(layer) => {
            layer.asked = now;
            if layer.inputs != inputs {
                layer.inputs = inputs;
                layer.dirty = true;
            }
            layer.dirty
        }
        None => {
            jobs.layers
                .insert(key, Layer { source: source.clone(), size, inputs, dirty: true, asked: now, frame: None });
            true
        }
    };
    if fresh && jobs.gpu != Gpu::Missing {
        wake_up(&mut jobs);
    }
    key
}

/// The newest frame made for a layer since the last one taken.
pub fn take(key: u64) -> Option<Frame> {
    JOBS.lock().layers.get_mut(&key).and_then(|l| l.frame.take())
}

/// Whether a frame for a layer is still being made.
pub fn waiting(key: u64) -> bool {
    let jobs = JOBS.lock();
    matches!(jobs.gpu, Gpu::Starting | Gpu::Ready) && jobs.layers.get(&key).is_some_and(|l| l.dirty)
}

/// What the GPU's compiler says about a custom shader: nothing when it
/// compiles, else its errors at the shader's own lines. None without a GPU.
pub fn check(code: String) -> oneshot::Receiver<Option<Vec<Diagnostic>>> {
    let (tx, rx) = oneshot::channel();
    if let Some(problem) = shader_problem(&code) {
        let _ = tx.send(Some(vec![Diagnostic { message: problem, line: None, column: None }]));
        return rx;
    }
    let mut jobs = JOBS.lock();
    if jobs.gpu == Gpu::Missing {
        let _ = tx.send(None);
        return rx;
    }
    jobs.checks.push((code, tx));
    wake_up(&mut jobs);
    rx
}

/// Lets a custom shader be compiled and timed afresh (after "Try it again"),
/// and a GPU lost to it be looked for again.
pub fn forget(id: &str) {
    let mut jobs = JOBS.lock();
    jobs.forget.push(id.to_owned());
    if jobs.gpu == Gpu::Missing && jobs.again {
        jobs.gpu = Gpu::Unknown;
        jobs.again = false;
    }
}

/// No GPU from here on: every check gets None and the window draws by hand.
/// `again` when it was lost rather than never there.
fn give_up(again: bool) {
    let checks = {
        let mut jobs = JOBS.lock();
        jobs.gpu = Gpu::Missing;
        jobs.again = again;
        for layer in jobs.layers.values_mut() {
            layer.dirty = false;
        }
        std::mem::take(&mut jobs.checks)
    };
    for (_, tx) in checks {
        let _ = tx.send(None);
    }
}

// ───────────────────────── The thread ─────────────────────────

/// What went wrong while drawing.
enum Trouble {
    /// A frame didn't come back in time, or the device was lost: this device is done.
    Stopped,
    /// The GPU refused what it was asked to draw.
    Rejected(String),
}

fn run() {
    let Some(mut gpu) = Context::new() else {
        tracing::info!("no GPU adapter for the backdrop's effects; they're drawn by hand");
        give_up(false);
        return;
    };
    JOBS.lock().gpu = Gpu::Ready;
    loop {
        let (checks, work, forget) = {
            let mut jobs = JOBS.lock();
            loop {
                if jobs.gpu != Gpu::Ready {
                    return;
                }
                let now = Instant::now();
                jobs.layers.retain(|_, l| now - l.asked < FORGET);
                if !jobs.checks.is_empty() || jobs.layers.values().any(|l| l.dirty) || !jobs.forget.is_empty() {
                    break;
                }
                WAKE.wait_for(&mut jobs, Duration::from_secs(1));
            }
            let work: Vec<(u64, Source, (u32, u32), Inputs)> = jobs
                .layers
                .iter_mut()
                .filter(|(_, l)| l.dirty)
                .map(|(k, l)| {
                    l.dirty = false;
                    (*k, l.source.clone(), l.size, l.inputs)
                })
                .collect();
            gpu.targets.retain(|k, _| jobs.layers.contains_key(k));
            (std::mem::take(&mut jobs.checks), work, std::mem::take(&mut jobs.forget))
        };
        for id in forget {
            gpu.customs.remove(&id);
        }
        for (code, tx) in checks {
            let _ = tx.send(Some(gpu.check(&code)));
        }
        for (key, source, size, inputs) in work {
            match gpu.draw(key, &source, size, inputs) {
                Ok(Some(frame)) => {
                    if let Some(layer) = JOBS.lock().layers.get_mut(&key) {
                        layer.frame = Some(frame);
                    }
                }
                Ok(None) => {}
                Err(Trouble::Stopped) => {
                    if let Source::Custom(code) = &source {
                        status::set_status(&shader_id(code), Some(ShaderStatus::Stopped));
                    }
                    tracing::warn!("the GPU stopped while drawing the backdrop; effects are drawn by hand now");
                    give_up(true);
                    // A stuck device may never let go: leave it be rather than wait on it.
                    std::mem::forget(gpu);
                    return;
                }
                Err(Trouble::Rejected(message)) => match &source {
                    Source::Custom(code) => {
                        let id = shader_id(code);
                        gpu.customs.remove(&id);
                        tracing::info!("the GPU refused a custom shader: {message}");
                        status::set_status(
                            &id,
                            Some(ShaderStatus::Broken { message: t("system.shader.gpuRejected"), line: None }),
                        );
                    }
                    Source::Effect(_) => {
                        tracing::warn!("the GPU refused a built-in effect: {message}");
                        give_up(false);
                        return;
                    }
                },
            }
        }
    }
}

/// A custom shader, compiled and timed.
struct Custom {
    pipeline: wgpu::RenderPipeline,
    scale: f32,
    frames: u32,
    slow: u32,
}

/// Where a layer is drawn and read back from.
struct Target {
    size: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    row: u32,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pointer: [f32; 2],
    last: Option<Instant>,
}

struct Context {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    builtins: HashMap<Effect, wgpu::RenderPipeline>,
    customs: HashMap<String, Custom>,
    targets: HashMap<u64, Target>,
    lost: Arc<AtomicBool>,
    errors: Arc<Mutex<Option<String>>>,
}

/// The first line a compiler message is about, without wgpu's and naga's framing.
fn short(message: &str) -> String {
    let lines: Vec<&str> = message.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let main = lines.iter().find_map(|l| l.split_once("error: ").map(|(_, m)| m)).or(lines.first().copied());
    let note = lines.iter().find_map(|l| l.strip_prefix("= "));
    match (main, note) {
        (Some(m), Some(n)) => format!("{m}: {n}"),
        (Some(m), None) => m.to_owned(),
        _ => t("system.shader.doesNotCompile"),
    }
}

impl Context {
    fn new() -> Option<Self> {
        // The native APIs first; GL only where none of them has an adapter.
        let from_env = std::env::var_os("WGPU_BACKEND").is_some();
        let tries: &[wgpu::Backends] =
            if from_env { &[wgpu::Backends::all()] } else { &[wgpu::Backends::PRIMARY, wgpu::Backends::GL] };
        let adapter = tries.iter().find_map(|backends| {
            let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
            if !from_env {
                desc.backends = *backends;
            }
            let instance = wgpu::Instance::new(desc);
            futures::executor::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
            .ok()
        })?;
        let info = adapter.get_info();
        let (device, queue) = futures::executor::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("fuwa backdrop"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .ok()?;
        tracing::info!("backdrop effects draw on {} ({:?})", info.name, info.backend);
        let lost = Arc::new(AtomicBool::new(false));
        device.set_device_lost_callback({
            let lost = lost.clone();
            move |reason, _| {
                if reason != wgpu::DeviceLostReason::Destroyed {
                    lost.store(true, Ordering::Relaxed);
                }
            }
        });
        let errors = Arc::new(Mutex::new(None));
        device.on_uncaptured_error(Arc::new({
            let errors = errors.clone();
            move |error: wgpu::Error| *errors.lock() = Some(error.to_string())
        }));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fuwa backdrop inputs"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fuwa backdrop"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        Some(Self {
            device,
            queue,
            layout,
            pipeline_layout,
            builtins: HashMap::new(),
            customs: HashMap::new(),
            targets: HashMap::new(),
            lost,
            errors,
        })
    }

    /// A pipeline for a fragment shader module (with vgpu's vertex shader
    /// after it), or what the compiler said about its source.
    fn compile(&self, source: &str) -> Result<wgpu::RenderPipeline, Vec<Found>> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fuwa effect"),
            source: wgpu::ShaderSource::Wgsl(format!("{source}\n{}", shaders::VERTEX).into()),
        });
        let info = futures::executor::block_on(module.get_compilation_info());
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fuwa effect"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vgpu_fullscreen_vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let error = futures::executor::block_on(scope.pop());
        let mut found: Vec<_> = info
            .messages
            .iter()
            .filter(|m| m.message_type == wgpu::CompilationMessageType::Error)
            .map(|m| (short(&m.message), m.location.map(|l| l.line_number), m.location.map(|l| l.line_position)))
            .collect();
        if found.is_empty()
            && let Some(error) = error
        {
            found.push((short(&error.to_string()), None, None));
        }
        if found.is_empty() { Ok(pipeline) } else { Err(found) }
    }

    fn check(&self, code: &str) -> Vec<Diagnostic> {
        match self.compile(&full_source(code)) {
            Ok(_) => Vec::new(),
            Err(found) => found
                .into_iter()
                .map(|(message, line, column)| {
                    let line = line.and_then(|l| shader_line(l as usize, code));
                    Diagnostic { message, column: line.and(column.map(|c| c as usize)), line }
                })
                .collect(),
        }
    }

    /// Draws a frame of a layer and reads it back; None while a custom shader can't draw.
    fn draw(&mut self, key: u64, source: &Source, view: (u32, u32), inputs: Inputs) -> Result<Option<Frame>, Trouble> {
        let id = match source {
            Source::Custom(code) => {
                let id = shader_id(code);
                if status::status(&id).is_some_and(|s| !matches!(s, ShaderStatus::Running { .. })) {
                    return Ok(None);
                }
                if !self.customs.contains_key(&id) && !self.prepare(key, code, view, inputs)? {
                    return Ok(None);
                }
                Some(id)
            }
            Source::Effect(effect) => {
                if !self.builtins.contains_key(effect) {
                    let Some(wgsl) = shaders::source(*effect) else { return Ok(None) };
                    let pipeline = self.compile(&wgsl).map_err(|e| Trouble::Rejected(format!("{e:?}")))?;
                    self.builtins.insert(*effect, pipeline);
                }
                None
            }
        };
        let scale = match (source, &id) {
            (Source::Effect(effect), _) => shaders::resolution(*effect),
            (_, Some(id)) => self.customs[id].scale,
            _ => 1.0,
        };
        let size = fit(view, scale);
        let (frame, ms) = self.frame(key, source, size, inputs, true)?;
        // Every couple of seconds, how long a custom shader's frame takes; three slow ones in a row and it drops a step.
        if let Some(id) = id
            && let Some(custom) = self.customs.get_mut(&id)
        {
            custom.frames += 1;
            if custom.frames.is_multiple_of(FPS * 2) {
                custom.slow = if ms > BUDGET_MS * 2.0 { custom.slow + 1 } else { 0 };
                if custom.slow >= 3 {
                    custom.slow = 0;
                    match CUSTOM_SCALES.iter().position(|s| *s == custom.scale).and_then(|n| CUSTOM_SCALES.get(n + 1)) {
                        Some(next) => {
                            custom.scale = *next;
                            status::set_status(&id, Some(ShaderStatus::Running { scale: *next, ms }));
                        }
                        None => {
                            self.customs.remove(&id);
                            status::set_status(&id, Some(ShaderStatus::Slow { ms }));
                            return Ok(None);
                        }
                    }
                }
            }
        }
        Ok(frame)
    }

    /// Checks and compiles a custom shader and times its first frames; true once it may draw.
    fn prepare(&mut self, key: u64, code: &str, view: (u32, u32), inputs: Inputs) -> Result<bool, Trouble> {
        let id = shader_id(code);
        if let Some(problem) = shader_problem(code) {
            status::set_status(&id, Some(ShaderStatus::Broken { message: problem, line: None }));
            return Ok(false);
        }
        let pipeline = match self.compile(&full_source(code)) {
            Ok(pipeline) => pipeline,
            Err(found) => {
                let (message, line, _) = found.into_iter().next().unwrap_or_default();
                let line = line.and_then(|l| shader_line(l as usize, code));
                status::set_status(&id, Some(ShaderStatus::Broken { message, line }));
                return Ok(false);
            }
        };
        self.customs.insert(id.clone(), Custom { pipeline, scale: CUSTOM_SCALES[0], frames: 0, slow: 0 });
        // Until its first frames come back, a shader that hangs the GPU (or the app) is remembered as the cause.
        status::start_trying(&id);
        let source = Source::Custom(code.to_owned());
        let mut ms = f32::INFINITY;
        for scale in CUSTOM_SCALES {
            if let Some(custom) = self.customs.get_mut(&id) {
                custom.scale = scale;
            }
            let size = fit(view, scale);
            ms = f32::INFINITY;
            for _ in 0..3 {
                ms = ms.min(self.frame(key, &source, size, inputs, false)?.1);
            }
            if ms <= BUDGET_MS {
                break;
            }
        }
        status::done_trying(&id);
        if ms > BUDGET_MS {
            self.customs.remove(&id);
            status::set_status(&id, Some(ShaderStatus::Slow { ms }));
            return Ok(false);
        }
        let scale = self.customs.get(&id).map_or(CUSTOM_SCALES[0], |c| c.scale);
        status::set_status(&id, Some(ShaderStatus::Running { scale, ms }));
        Ok(true)
    }

    /// The target for a layer, made (again) at `size`.
    fn target(&mut self, key: u64, size: (u32, u32)) -> &mut Target {
        if self.targets.get(&key).is_none_or(|t| t.size != size) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("fuwa backdrop frame"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let row = (size.0 * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fuwa backdrop readback"),
                size: u64::from(row) * u64::from(size.1),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fuwa backdrop inputs"),
                size: UNIFORM_BYTES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fuwa backdrop inputs"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
            });
            let (pointer, last) = self.targets.get(&key).map_or(([0.5, 0.5], None), |t| (t.pointer, t.last));
            self.targets.insert(key, Target { size, texture, view, readback, row, uniform, bind_group, pointer, last });
        }
        self.targets.get_mut(&key).expect("made above")
    }

    /// Draws one frame into a layer's target, and reads it back when `read`.
    /// Gives the frame and how long the GPU took, in milliseconds.
    fn frame(
        &mut self,
        key: u64,
        source: &Source,
        size: (u32, u32),
        inputs: Inputs,
        read: bool,
    ) -> Result<(Option<Frame>, f32), Trouble> {
        let target = self.target(key, size);
        let now = Instant::now();
        let dt = target.last.map_or(0.0, |l| (now - l).as_secs_f32());
        target.last = Some(now);
        if let Some([x, y]) = inputs.pointer {
            let k = if inputs.still { 1.0 } else { 1.0 - (-dt * 7.0).exp() };
            target.pointer[0] += (x - target.pointer[0]) * k;
            target.pointer[1] += (y - target.pointer[1]) * k;
        }
        let bytes = uniform(source, size, target.pointer, &inputs);
        let (view, readback, row, texture) =
            (target.view.clone(), target.readback.clone(), target.row, target.texture.clone());
        let bind_group = target.bind_group.clone();
        let buffer = target.uniform.clone();
        let pipeline = match source {
            Source::Effect(effect) => self.builtins.get(effect),
            Source::Custom(code) => self.customs.get(&shader_id(code)).map(|c| &c.pipeline),
        };
        let Some(pipeline) = pipeline else { return Ok((None, 0.0)) };
        self.queue.write_buffer(&buffer, 0, &bytes);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fuwa backdrop"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        if read {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(size.1),
                    },
                },
                wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            );
        }
        let start = Instant::now();
        let submitted = self.queue.submit([encoder.finish()]);
        let waited = self.device.poll(wgpu::PollType::Wait { submission_index: Some(submitted), timeout: Some(STUCK) });
        let ms = start.elapsed().as_secs_f32() * 1000.0;
        if waited.is_err() || self.lost.load(Ordering::Relaxed) {
            return Err(Trouble::Stopped);
        }
        if let Some(error) = self.errors.lock().take() {
            return Err(Trouble::Rejected(error));
        }
        if !read {
            return Ok((None, ms));
        }
        let slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(STUCK) });
        if !matches!(rx.recv_timeout(STUCK), Ok(Ok(()))) {
            return Err(Trouble::Stopped);
        }
        let bgra = straight_bgra(&slice.get_mapped_range(), size, row);
        readback.unmap();
        Ok((Some(Frame { width: size.0, height: size.1, bgra }), ms))
    }
}

/// A compiler message, at a line and column of the full source.
type Found = (String, Option<u32>, Option<u32>);

/// The uniform's size: the custom shaders' `Fuwa` (88 bytes, padded to 16); the built-ins' `Params` is 64.
const UNIFORM_BYTES: usize = 96;

/// The inputs as the shader's uniform: `Params` for a built-in effect, `Fuwa` for a custom shader.
fn uniform(source: &Source, size: (u32, u32), pointer: [f32; 2], inputs: &Inputs) -> [u8; UNIFORM_BYTES] {
    let [c1, c2, c3, c4] = inputs.colors;
    let res = [size.0 as f32, size.1 as f32];
    let mut values: Vec<f32> = Vec::with_capacity(UNIFORM_BYTES / 4);
    match source {
        Source::Effect(_) => {
            values.extend(c1.into_iter().chain(c2).chain(c3).chain(res));
            values.extend([inputs.time, inputs.intensity]);
        }
        Source::Custom(_) => {
            values.extend(c1.into_iter().chain(c2).chain(c3).chain(c4).chain(res).chain(pointer));
            values.extend([inputs.time, inputs.intensity]);
        }
    }
    let mut out = [0u8; UNIFORM_BYTES];
    for (chunk, v) in out.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        chunk.copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// A frame's size for a layer `view` pixels big at `scale` of the full resolution.
fn fit(view: (u32, u32), scale: f32) -> (u32, u32) {
    let long = view.0.max(view.1).max(1) as f32;
    let k = (FULL_SIDE / long).min(1.0) * scale;
    let side = |n: u32| ((n as f32 * k).round() as u32).max(1);
    (side(view.0), side(view.1))
}

/// Premultiplied RGBA rows (each `row` bytes apart) as the straight-alpha BGRA GPUI's pictures take.
fn straight_bgra(data: &[u8], size: (u32, u32), row: u32) -> Vec<u8> {
    let (w, h, row) = (size.0 as usize, size.1 as usize, row as usize);
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for p in data[y * row..y * row + w * 4].as_chunks::<4>().0 {
            let a = u32::from(p[3]);
            if a == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let c = |v: u8| ((u32::from(v) * 255 + a / 2) / a).min(255) as u8;
            out.extend_from_slice(&[c(p[2]), c(p[1]), c(p[0]), p[3]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_small_and_keep_their_shape() {
        assert_eq!(fit((2560, 1600), 1.0), (640, 400));
        assert_eq!(fit((2560, 1600), 0.5), (320, 200));
        assert_eq!(fit((200, 56), 1.0), (200, 56));
        assert_eq!(fit((0, 0), 0.34), (1, 1));
    }

    #[test]
    fn frames_turn_straight_and_bgra() {
        // Half-covered red, padded rows.
        let data = [128, 0, 0, 128, 0, 0, 0, 0, 9, 9, 9, 9];
        assert_eq!(straight_bgra(&data, (1, 1), 12), vec![0, 0, 255, 128]);
        assert_eq!(straight_bgra(&data[4..], (1, 1), 4), vec![0, 0, 0, 0]);
    }

    #[test]
    fn inputs_line_up_with_the_structs() {
        let inputs = Inputs { colors: [[1.0; 4]; 4], time: 7.0, intensity: 0.5, pointer: None, still: false };
        let f = |b: &[u8], i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        let p = uniform(&Source::Effect(Effect::Aurora), (640, 400), [0.5, 0.5], &inputs);
        assert_eq!((f(&p, 12), f(&p, 13), f(&p, 14), f(&p, 15)), (640.0, 400.0, 7.0, 0.5));
        let c = uniform(&Source::Custom(String::new()), (640, 400), [0.25, 0.75], &inputs);
        assert_eq!((f(&c, 16), f(&c, 18), f(&c, 19), f(&c, 20), f(&c, 21)), (640.0, 0.25, 0.75, 7.0, 0.5));
    }

    /// Waits for a layer's frame, or None once there's clearly no GPU.
    fn frame_of(source: &Source, size: (u32, u32)) -> Option<Frame> {
        let inputs =
            Inputs { colors: [[0.94, 0.38, 0.57, 1.0]; 4], time: 40.0, intensity: 1.0, pointer: None, still: true };
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(60) {
            let key = request(source, size, inputs);
            if let Some(frame) = take(key) {
                return Some(frame);
            }
            if gpu() == Gpu::Missing {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no frame in a minute");
    }

    #[test]
    fn draws_built_in_and_custom_shaders_where_there_is_a_gpu() {
        let Some(frame) = frame_of(&Source::Effect(Effect::Waves), (800, 500)) else {
            eprintln!("no GPU adapter here; skipping");
            return;
        };
        // Waves at three quarters of the full side, and something drawn.
        assert_eq!((frame.width, frame.height), (480, 300));
        assert!(frame.bgra.chunks(4).any(|p| p[3] > 0));
        // The lower part has the waves, the top is clear.
        assert_eq!(frame.bgra[3], 0);
        assert!(frame.bgra[(299 * 480 + 10) * 4 + 3] > 0);

        let plasma = crate::core::effects::custom::STARTERS[1].code.to_owned();
        let frame = frame_of(&Source::Custom(plasma.clone()), (800, 500)).expect("a frame");
        assert!(matches!(status::status(&shader_id(&plasma)), Some(ShaderStatus::Running { .. })));
        assert!(frame.bgra.chunks(4).all(|p| p[3] >= 101));

        // The compiler's errors land on the shader's own lines.
        let broken = "fn shade(uv: vec2f) -> vec4f {\n  let x = ;\n  return vec4f(0.0);\n}\n".to_owned();
        let found = futures::executor::block_on(check(broken)).unwrap().unwrap();
        assert_eq!(found[0].line, Some(2), "{found:?}");
        let typo = "fn shade(uv: vec2f) -> vec4f {\n  return vec4f(nope);\n}\n".to_owned();
        let found = futures::executor::block_on(check(typo)).unwrap().unwrap();
        assert_eq!(found[0].line, Some(2), "{found:?}");
        assert!(futures::executor::block_on(check(plasma)).unwrap().unwrap().is_empty());
    }

    #[test]
    fn compiler_messages_are_short() {
        let naga = "\nShader 'fuwa effect' parsing error: expected expression, found ';'\n  ┌─ wgsl:40:11\n  │\n40 │   let x = ;\n  │           ^ expected expression\n";
        assert_eq!(short(naga), "expected expression, found ';'");
    }
}
