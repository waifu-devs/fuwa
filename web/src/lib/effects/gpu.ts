import { effect, frame as openFrame, frameLoop, init, surface, type Effect, type Frame, type FrameLoopHandle, type Gpu, type Surface } from "vgpu";
import type { ShaderEffect } from "@/lib/backdrop";
import { fullSource, shaderId, shaderLine, shaderProblem } from "@/lib/effects/custom";
import { RESOLUTION, SHADERS } from "@/lib/effects/shaders";
import { setShaderStatus, shaderStatus, startTrying } from "@/lib/effects/status";
import { i18n } from "@/i18n/i18n";

/**
 * Draws a shader effect into a canvas with vgpu (WebGPU). This module is
 * loaded only when someone picks a shader effect on a device with WebGPU, so
 * vgpu stays out of the app's first download.
 *
 * Costs are kept small: one fullscreen pass, at most 30 frames a second, at
 * half resolution for the smooth effects, and nothing at all while the window
 * is hidden or the canvas is paused. Per frame only a few numbers change.
 *
 * Custom shaders (lib/effects/custom.ts) get more care, since anyone can
 * write one: they're checked and compiled before they draw, their first
 * frames are timed (at three quarters, then half, then a third of the
 * resolution until one fits), they're timed again every couple of seconds
 * while they run, and a GPU that's lost while one draws is blamed on it. Any
 * of these ends in `onTrouble`, and the backdrop shows the shader's fallback.
 */

export type Colors = { c1: number[]; c2: number[]; c3: number[]; c4: number[] };

/** What to draw: a built-in effect, or the WGSL of a custom shader. */
export type Source = { effect: ShaderEffect } | { custom: string };

export type PainterOptions = { source: Source; intensity: number; speed: number; colors: Colors; still: boolean };

export type Painter = {
  set(options: PainterOptions): void;
  /** Whether it may draw at all (false while hidden, or covered by settings). */
  setRunning(running: boolean): void;
  dispose(): void;
};

/** Frames a second: plenty for slow ambient motion, half a 60 Hz display's work. */
const FPS = 30;
/** Resolutions a custom shader is tried at, until its frames fit the budget. */
const CUSTOM_SCALES = [0.75, 0.5, 0.34];
/** A custom shader's frame may take this long on the GPU (a 30 fps frame is 33 ms, and the app draws too). */
const BUDGET_MS = 20;
/** A frame that hasn't come back after this long means the shader is stuck. */
const STUCK_MS = 2500;

let shared: Promise<Gpu | null> | null = null;

/** One GPU device for the whole app, or null where WebGPU isn't there or won't start. */
export function gpu(): Promise<Gpu | null> {
  shared ??= (async () => {
    if (typeof navigator === "undefined" || !("gpu" in navigator)) return null;
    try {
      const device = await init({ powerPreference: "low-power", label: "fuwa backdrop" });
      // A lost device (a driver reset, the GPU process gone) is replaced next time it's asked for.
      void device.gpu.lost.then(() => {
        shared = null;
      });
      return device;
    } catch {
      return null;
    }
  })();
  return shared;
}

export type Diagnostic = { message: string; line: number | null; column: number | null };

/**
 * What the GPU's compiler says about a custom shader: nothing when it
 * compiles, else its errors at the shader's own lines. Null without WebGPU.
 */
export async function checkShader(code: string): Promise<Diagnostic[] | null> {
  const problem = shaderProblem(i18n().t, code);
  if (problem) return [{ message: problem, line: null, column: null }];
  const device = await gpu();
  if (!device) return null;
  try {
    const module = device.gpu.createShaderModule({ code: fullSource(code), label: "fuwa custom shader check" });
    const info = await module.getCompilationInfo();
    return info.messages
      .filter((m) => m.type === "error")
      .map((m) => {
        const line = shaderLine(m.lineNum, code);
        return { message: m.message, line, column: line === null ? null : m.linePos };
      });
  } catch (err) {
    return [{ message: (err as Error).message || i18n().t("system.shader.doesNotCompile"), line: null, column: null }];
  }
}

const sleep = (ms: number) => new Promise<"late">((resolve) => setTimeout(() => resolve("late"), ms));

/** A painter for a canvas, or null without WebGPU. `onLost` is called if the GPU goes away later, to fall back. */
export async function createPainter(canvas: HTMLCanvasElement, onLost: () => void, onTrouble: (id: string) => void = () => {}): Promise<Painter | null> {
  const device = await gpu();
  if (!device) return null;
  let target: Surface;
  try {
    target = surface(device, canvas, { clearColor: [0, 0, 0, 0], alphaMode: "premultiplied", autoResize: false, size: [1, 1] });
  } catch {
    return null;
  }
  const builtins = new Map<ShaderEffect, Effect>();
  /** The custom shader in use: compiled and timed (`ready`), or on its way. */
  let custom: { id: string; code: string; fx: Effect | null; ready: boolean; scale: number; pointer: boolean; slow: number } | null = null;
  let current: PainterOptions | null = null;
  let running = false;
  let loop: FrameLoopHandle | null = null;
  let disposed = false;
  let lost = false;
  // The effect's own clock, so changing the speed doesn't jump.
  let time = 0;
  let last = 0;
  let frames = 0;
  // The pointer, in the canvas's 0..1, eased toward where it really is.
  const pointer = { x: 0.5, y: 0.5, toX: 0.5, toY: 0.5, seen: false };
  const onPointer = (e: PointerEvent) => {
    pointer.toX = e.clientX;
    pointer.toY = e.clientY;
    pointer.seen = true;
  };

  const builtin = (name: ShaderEffect) => {
    let fx = builtins.get(name);
    if (!fx) {
      fx = effect(device, SHADERS[name], {
        label: name,
        set: { params: { c1: [0, 0, 0, 1], c2: [0, 0, 0, 1], c3: [0, 0, 0, 1], res: [1, 1], time: 0, intensity: 0 } },
      });
      builtins.set(name, fx);
    }
    return fx;
  };

  const resolution = () => {
    if (!current) return 1;
    return "effect" in current.source ? RESOLUTION[current.source.effect] : (custom?.scale ?? CUSTOM_SCALES[0]!);
  };

  const size = () => {
    const scale = Math.min(window.devicePixelRatio || 1, 1.5) * resolution();
    const width = Math.max(1, Math.round(canvas.clientWidth * scale));
    const height = Math.max(1, Math.round(canvas.clientHeight * scale));
    if (target.size[0] !== width || target.size[1] !== height) target.resize([width, height]);
  };

  /** The pass for this frame, with its inputs set; null while a custom shader isn't ready. */
  const prepare = (dt: number) => {
    if (!current) return null;
    const { colors, intensity } = current;
    const res = [target.size[0], target.size[1]];
    if ("effect" in current.source) {
      const fx = builtin(current.source.effect);
      return fx.set({ params: { c1: colors.c1, c2: colors.c2, c3: colors.c3, res, time, intensity: intensity / 100 } });
    }
    if (!custom?.fx || !custom.ready) return null;
    if (custom.pointer && pointer.seen) {
      const rect = canvas.getBoundingClientRect();
      const toX = (pointer.toX - rect.left) / Math.max(rect.width, 1);
      const toY = (pointer.toY - rect.top) / Math.max(rect.height, 1);
      const k = current.still ? 1 : 1 - Math.exp(-dt * 7);
      pointer.x += (toX - pointer.x) * k;
      pointer.y += (toY - pointer.y) * k;
    }
    return custom.fx.set({
      fuwa: {
        primary: colors.c1,
        accent: colors.c2,
        background: colors.c3,
        foreground: colors.c4,
        res,
        pointer: [pointer.x, pointer.y],
        time,
        intensity: intensity / 100,
      },
    });
  };

  const draw = (frame?: Frame) => {
    if (!current) return;
    size();
    const now = performance.now();
    const dt = last ? (now - last) / 1000 : 0;
    if (last) time += dt * (current.speed / 100);
    last = now;
    const fx = prepare(dt);
    if (!fx) return;
    // A surface is drawn inside a frame: the loop's, or one of its own for a single still frame.
    if (frame) frame.pass(target, fx);
    else openFrame(device, (f) => f.pass(target, fx));
    // Every couple of seconds, how long a custom shader's frame takes on the GPU.
    if (frame && custom?.ready && ++frames % (FPS * 2) === 0) setTimeout(() => void watch(custom), 0);
  };

  /** One frame's time on the GPU, from submitting it to it being done. */
  const timeFrame = async (fx: Effect) => {
    const start = performance.now();
    openFrame(device, (f) => f.pass(target, fx));
    const done = await Promise.race([device.gpu.queue.onSubmittedWorkDone(), sleep(STUCK_MS)]);
    return done === "late" ? Infinity : performance.now() - start;
  };

  /** Times a running custom shader; three slow samples in a row and it drops a step of resolution, or stops. */
  const watch = async (watched: typeof custom) => {
    if (!watched || watched !== custom || disposed) return;
    const start = performance.now();
    const done = await Promise.race([device.gpu.queue.onSubmittedWorkDone(), sleep(STUCK_MS)]);
    const ms = done === "late" ? Infinity : performance.now() - start;
    if (watched !== custom) return;
    watched.slow = ms > BUDGET_MS * 2 ? watched.slow + 1 : 0;
    if (watched.slow < 3) return;
    watched.slow = 0;
    const next = CUSTOM_SCALES[CUSTOM_SCALES.indexOf(watched.scale) + 1];
    if (next) {
      watched.scale = next;
      setShaderStatus(watched.id, { state: "running", scale: next, ms });
    } else {
      trouble(watched.id, { state: "slow", ms });
    }
  };

  const trouble = (id: string, status: Parameters<typeof setShaderStatus>[1]) => {
    setShaderStatus(id, status);
    if (custom?.id === id) custom = null;
    stopLoop();
    if (!disposed) onTrouble(id);
  };

  /** Compiles a custom shader and times its first frames, then lets it draw. */
  const prepareCustom = async (code: string) => {
    const id = shaderId(code);
    custom = null;
    const known = shaderStatus(id);
    if (known && known.state !== "running") return onTrouble(id);
    const problem = shaderProblem(i18n().t, code);
    if (problem) return trouble(id, { state: "broken", message: problem, line: null });
    const mine = { id, code, fx: null as Effect | null, ready: false, scale: CUSTOM_SCALES[0]!, pointer: /\bfuwa\s*\.\s*pointer\b/.test(code), slow: 0 };
    custom = mine;
    let fx: Effect;
    try {
      fx = effect(device, fullSource(code), { label: "custom" });
      await fx.compile({ colors: [target.format] });
    } catch {
      const errors = await checkShader(code);
      const first = errors?.[0];
      return mine === custom && trouble(id, { state: "broken", message: first?.message ?? i18n().t("system.shader.doesNotCompileHere"), line: first?.line ?? null });
    }
    if (mine !== custom || disposed) return;
    mine.fx = fx;
    mine.ready = true;
    // Until its first frames come back, a shader that hangs the GPU (or the tab) is remembered as the cause.
    const done = startTrying(id);
    let ms = Infinity;
    for (const scale of CUSTOM_SCALES) {
      mine.scale = scale;
      size();
      prepare(0);
      ms = Math.min(await timeFrame(fx), await timeFrame(fx), await timeFrame(fx));
      if (mine !== custom || disposed) return done(!lost);
      if (ms === Infinity) return trouble(id, { state: "stopped" });
      if (ms <= BUDGET_MS) break;
    }
    done(true);
    if (ms > BUDGET_MS) return trouble(id, { state: "slow", ms });
    setShaderStatus(id, { state: "running", scale: mine.scale, ms });
    sync();
  };

  /** A custom shader set while the painter couldn't draw, tried once it can. */
  let waiting: string | null = null;
  const startWaiting = () => {
    const code = waiting!;
    waiting = null;
    void prepareCustom(code);
  };

  const stopLoop = () => {
    loop?.stop();
    loop = null;
  };

  const sync = () => {
    const ready = !!current && ("effect" in current.source || !!custom?.ready);
    const animate = running && ready && !current!.still && current!.speed > 0;
    if (animate && !loop) {
      last = 0;
      loop = frameLoop(device, (frame) => draw(frame), { fps: FPS });
    } else if (!animate && loop) {
      stopLoop();
    }
    if (custom?.pointer && animate) window.addEventListener("pointermove", onPointer, { passive: true });
    else window.removeEventListener("pointermove", onPointer);
    // Still (reduced motion, or speed 0): one frame, then nothing.
    if (!animate && running && ready) draw();
  };

  void device.gpu.lost.then(() => {
    lost = true;
    stopLoop();
    if (disposed) return;
    // Lost while a custom shader drew: it's the likely cause, so it isn't run again on its own.
    if (custom) setShaderStatus(custom.id, { state: "stopped" });
    custom = null;
    onLost();
  });
  // Errors while drawing (a shader the driver rejects) also mean the CSS version is the better choice.
  const stopErrors = device.onError(() => {
    stopLoop();
    if (disposed) return;
    if (custom) return trouble(custom.id, { state: "broken", message: i18n().t("system.shader.gpuRejected"), line: null });
    onLost();
  });

  return {
    set(options) {
      const code = "custom" in options.source ? options.source.custom : null;
      current = options;
      if (code !== null && code !== custom?.code && code !== waiting) {
        // Tried only once it may draw: a backdrop under settings doesn't spend the GPU on it.
        custom = null;
        waiting = code;
        stopLoop();
        if (running) startWaiting();
        return;
      }
      if (code === null) custom = waiting = null;
      sync();
    },
    setRunning(next) {
      if (next === running) return;
      running = next;
      if (running && waiting !== null) return startWaiting();
      sync();
    },
    dispose() {
      disposed = true;
      custom = null;
      stopErrors();
      stopLoop();
      window.removeEventListener("pointermove", onPointer);
      target.dispose();
    },
  };
}
