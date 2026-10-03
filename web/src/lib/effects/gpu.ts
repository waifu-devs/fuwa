import { effect, frameLoop, init, surface, type Effect, type Frame, type FrameLoopHandle, type Gpu, type Surface } from "vgpu";
import type { ShaderEffect } from "@/lib/backdrop";
import { RESOLUTION, SHADERS } from "@/lib/effects/shaders";

/**
 * Draws a shader effect into a canvas with vgpu (WebGPU). This module is
 * loaded only when someone picks a shader effect on a device with WebGPU, so
 * vgpu stays out of the app's first download.
 *
 * Costs are kept small: one fullscreen pass, at most 30 frames a second, at
 * half resolution for the smooth effects, and nothing at all while the window
 * is hidden or the canvas is paused. Per frame only two numbers change.
 */

export type Colors = { c1: number[]; c2: number[]; c3: number[] };

export type Painter = {
  set(options: { effect: ShaderEffect; intensity: number; speed: number; colors: Colors; still: boolean }): void;
  /** Whether it may draw at all (false while hidden, or covered by settings). */
  setRunning(running: boolean): void;
  dispose(): void;
};

/** Frames a second: plenty for slow ambient motion, half a 60 Hz display's work. */
const FPS = 30;

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

/** A painter for a canvas, or null without WebGPU. `onLost` is called if the GPU goes away later, to fall back. */
export async function createPainter(canvas: HTMLCanvasElement, onLost: () => void): Promise<Painter | null> {
  const device = await gpu();
  if (!device) return null;
  let target: Surface;
  try {
    target = surface(device, canvas, { clearColor: [0, 0, 0, 0], alphaMode: "premultiplied", autoResize: false, size: [1, 1] });
  } catch {
    return null;
  }
  const effects = new Map<ShaderEffect, Effect>();
  let current: { effect: ShaderEffect; intensity: number; speed: number; colors: Colors; still: boolean } | null = null;
  let running = false;
  let loop: FrameLoopHandle | null = null;
  // The effect's own clock, so changing the speed doesn't jump.
  let time = 0;
  let last = 0;

  const pass = (name: ShaderEffect) => {
    let fx = effects.get(name);
    if (!fx) {
      fx = effect(device, SHADERS[name], {
        label: name,
        set: { params: { c1: [0, 0, 0, 1], c2: [0, 0, 0, 1], c3: [0, 0, 0, 1], res: [1, 1], time: 0, intensity: 0 } },
      });
      effects.set(name, fx);
    }
    return fx;
  };

  const size = () => {
    if (!current) return;
    const scale = Math.min(window.devicePixelRatio || 1, 1.5) * RESOLUTION[current.effect];
    const width = Math.max(1, Math.round(canvas.clientWidth * scale));
    const height = Math.max(1, Math.round(canvas.clientHeight * scale));
    if (target.size[0] !== width || target.size[1] !== height) target.resize([width, height]);
  };

  const draw = (frame?: Frame) => {
    if (!current) return;
    size();
    const now = performance.now();
    if (last) time += ((now - last) / 1000) * (current.speed / 100);
    last = now;
    const fx = pass(current.effect);
    fx.set({ params: { ...current.colors, res: [target.size[0], target.size[1]], time, intensity: current.intensity / 100 } });
    if (frame) frame.pass(target, fx);
    else fx.draw(target);
  };

  const sync = () => {
    const animate = running && !!current && !current.still && current.speed > 0;
    if (animate && !loop) {
      last = 0;
      loop = frameLoop(device, (frame) => draw(frame), { fps: FPS });
    } else if (!animate && loop) {
      loop.stop();
      loop = null;
    }
    // Still (reduced motion, or speed 0): one frame, then nothing.
    if (!animate && running && current) draw();
  };

  let disposed = false;
  void device.gpu.lost.then(() => {
    loop?.stop();
    loop = null;
    if (!disposed) onLost();
  });
  // Errors while drawing (a shader the driver rejects) also mean the CSS version is the better choice.
  const stopErrors = device.onError(() => {
    loop?.stop();
    loop = null;
    if (!disposed) onLost();
  });

  return {
    set(options) {
      current = options;
      sync();
    },
    setRunning(next) {
      if (next === running) return;
      running = next;
      sync();
    },
    dispose() {
      disposed = true;
      stopErrors();
      loop?.stop();
      loop = null;
      target.dispose();
    },
  };
}
