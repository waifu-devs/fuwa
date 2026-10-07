/**
 * Profile effects: an animated layer over someone's profile card, like
 * petals drifting past or stars twinkling at the edges. Each effect is data
 * (`ProfileEffectSpec`), not code or pictures: layers of small shapes, each
 * with a way of moving. The app turns a spec into a plan of particles and
 * plays it with transform and opacity animations only, so the browser's
 * compositor does the work and the page's own thread stays free.
 *
 * An effect plays in two parts, the way Discord's do: a short, dense intro
 * when the card opens, then a sparse idle loop for as long as it stays open.
 * Particles keep to the card's edges and corners, so the name and bio stay
 * readable.
 *
 * Colors name theme tokens (`primary`, `ring`, …), `profile` for the card's
 * own color, or a fixed `#rrggbb`. Tokens are CSS variables, so an effect
 * follows whatever theme the viewer has on.
 *
 * Every app draws the same specs (docs/profile-effects.md has the format for
 * the desktop app and for custom effects later), and a profile names its
 * effect by id. An id the app doesn't know shows nothing.
 *
 * This file is pure (no DOM), so `node --test` covers it.
 */

export const SHAPES = ["petal", "star", "sparkle", "heart", "snowflake", "bubble", "dot", "confetti", "streak"] as const;
export type Shape = (typeof SHAPES)[number];

export const MOTIONS = ["fall", "rise", "twinkle", "drift", "burst", "shoot", "pop"] as const;
/**
 * How a particle moves:
 * - fall / rise: crosses the card top to bottom (or up), swaying.
 * - twinkle: grows and shrinks in place, then rests for the rest of its loop.
 * - drift: wanders around where it started, glowing brighter and dimmer.
 * - burst: flies out from its starting point and falls away (intros).
 * - shoot: streaks across, then rests (shooting stars).
 * - pop: swells in place and pops (intros).
 */
export type Motion = (typeof MOTIONS)[number];

export const REGIONS = ["top", "bottom", "edges", "corners", "anywhere", "top-left", "top-right", "center"] as const;
/** Where particles start. `edges` and `corners` keep the middle of the card clear. */
export type Region = (typeof REGIONS)[number];

export const PAINT_TOKENS = ["primary", "ring", "accent", "foreground", "card", "profile"] as const;
/** A theme token, `profile` (the card's color), or `#rrggbb`. */
export type Paint = (typeof PAINT_TOKENS)[number] | `#${string}`;

export type Range = [min: number, max: number];

export type EffectLayer = {
  shape: Shape;
  motion: Motion;
  /** Intro layers play once when the card opens; idle ones loop after. */
  phase: "intro" | "idle";
  /** Particles on a card of the reference size (MAX_PER_LAYER at most). */
  count: number;
  from: Region;
  /** Size in pixels. */
  size: Range;
  /** One pass, in milliseconds. */
  duration: Range;
  /** Intro layers: when each particle starts, in milliseconds after opening. */
  delay?: Range;
  colors: Paint[];
  /** Turns by up to this many degrees over a pass. */
  spin?: number;
  /** Sideways sway (fall, rise), wander radius (drift) or flight distance (burst), in pixels. */
  sway?: number;
  /** Tumbles in 3D as it moves, like paper. */
  flip?: boolean;
  /** A soft glow in the particle's color. */
  glow?: boolean;
  /** Peak opacity, 0 to 1. */
  opacity?: number;
};

export type ProfileEffectSpec = {
  id: string;
  name: string;
  /** One short line for the picker. */
  description: string;
  layers: EffectLayer[];
};

/** The card these counts and sizes are for: a profile popout. */
export const REFERENCE = { width: 304, height: 420 } as const;
export const MAX_LAYERS = 6;
export const MAX_PER_LAYER = 24;
/** Particles on screen at once for one card, after scaling. */
export const MAX_PARTICLES = 60;

const PINKS: Paint[] = ["#ffb7c5", "#ff8fab", "#ffc8d6", "primary"];
const GOLD: Paint[] = ["#fbbf24", "#fcd34d", "#f59e0b", "primary"];
const CONFETTI: Paint[] = ["primary", "ring", "#fbbf24", "#34d399", "#60a5fa", "#f472b6"];

/** The effects every fuwa app ships with. */
export const BUILTIN_EFFECTS: ProfileEffectSpec[] = [
  {
    id: "sakura",
    name: "Sakura",
    description: "A gust of petals, then a gentle fall.",
    layers: [
      { shape: "petal", motion: "burst", phase: "intro", count: 18, from: "top-left", size: [18, 28], duration: [1500, 2300], delay: [0, 260], colors: PINKS, spin: 540, sway: 280, flip: true, opacity: 0.95 },
      { shape: "petal", motion: "fall", phase: "idle", count: 9, from: "top", size: [15, 24], duration: [6500, 9500], colors: PINKS, spin: 320, sway: 22, flip: true, opacity: 0.9 },
    ],
  },
  {
    id: "starfall",
    name: "Starfall",
    description: "Stars blink on around you, with the odd shooting star.",
    layers: [
      { shape: "star", motion: "twinkle", phase: "intro", count: 15, from: "edges", size: [14, 24], duration: [1400, 2000], delay: [0, 700], colors: [...GOLD, "ring"], spin: 90, glow: true },
      { shape: "streak", motion: "shoot", phase: "intro", count: 1, from: "top", size: [76, 96], duration: [1400, 1400], delay: [250, 250], colors: ["#fcd34d"], glow: true },
      { shape: "sparkle", motion: "twinkle", phase: "idle", count: 11, from: "edges", size: [11, 19], duration: [2600, 4600], colors: [...GOLD, "ring"], spin: 60, glow: true },
      { shape: "streak", motion: "shoot", phase: "idle", count: 2, from: "top", size: [64, 96], duration: [6000, 9000], colors: ["#fcd34d", "primary"], glow: true },
    ],
  },
  {
    id: "sparkles",
    name: "Sparkles",
    description: "A pop of sparkles that keep glinting at the edges.",
    layers: [
      { shape: "sparkle", motion: "pop", phase: "intro", count: 14, from: "edges", size: [18, 32], duration: [900, 1500], delay: [0, 650], colors: ["primary", "#fbbf24", "ring"], spin: 90, glow: true },
      { shape: "sparkle", motion: "twinkle", phase: "idle", count: 10, from: "edges", size: [12, 24], duration: [2200, 3800], colors: ["primary", "#fbbf24", "ring"], spin: 90, glow: true },
    ],
  },
  {
    id: "hearts",
    name: "Hearts",
    description: "Hearts bubble up from below.",
    layers: [
      { shape: "heart", motion: "burst", phase: "intro", count: 13, from: "bottom", size: [18, 30], duration: [1300, 1900], delay: [0, 300], colors: ["#ff6b9d", "#fb7185", "profile", "primary"], spin: 40, sway: 260, opacity: 0.95 },
      { shape: "heart", motion: "rise", phase: "idle", count: 7, from: "bottom", size: [15, 24], duration: [5500, 8500], colors: ["#ff6b9d", "#fb7185", "profile", "primary"], spin: 30, sway: 14, opacity: 0.85 },
    ],
  },
  {
    id: "snow",
    name: "Snowfall",
    description: "A flurry, then soft snow.",
    layers: [
      { shape: "snowflake", motion: "fall", phase: "intro", count: 18, from: "top", size: [14, 22], duration: [1800, 2500], delay: [0, 450], colors: ["#ffffff", "#dbeafe", "#bfdbfe", "ring"], spin: 180, sway: 16, opacity: 0.95 },
      { shape: "dot", motion: "fall", phase: "idle", count: 12, from: "top", size: [5, 9], duration: [7000, 11000], colors: ["#ffffff", "#dbeafe"], sway: 14, opacity: 0.9 },
      { shape: "snowflake", motion: "fall", phase: "idle", count: 5, from: "top", size: [13, 20], duration: [8000, 12000], colors: ["#ffffff", "#bfdbfe", "ring"], spin: 200, sway: 18, opacity: 0.9 },
    ],
  },
  {
    id: "bubbles",
    name: "Bubbles",
    description: "Bubbles pop in and float away.",
    layers: [
      { shape: "bubble", motion: "pop", phase: "intro", count: 12, from: "edges", size: [20, 38], duration: [1000, 1600], delay: [0, 600], colors: ["ring", "#38bdf8", "primary"], opacity: 0.9 },
      { shape: "bubble", motion: "rise", phase: "idle", count: 8, from: "bottom", size: [14, 28], duration: [6000, 9500], colors: ["ring", "#38bdf8", "primary"], sway: 12, opacity: 0.8 },
    ],
  },
  {
    id: "fireflies",
    name: "Fireflies",
    description: "Little lights that wander and glow.",
    layers: [
      { shape: "dot", motion: "pop", phase: "intro", count: 10, from: "edges", size: [12, 18], duration: [900, 1300], delay: [0, 700], colors: ["#facc15", "#a3e635", "primary"], glow: true },
      { shape: "dot", motion: "drift", phase: "idle", count: 11, from: "edges", size: [11, 16], duration: [4500, 7500], colors: ["#facc15", "#a3e635", "primary"], sway: 26, glow: true },
    ],
  },
  {
    id: "confetti",
    name: "Confetti",
    description: "A burst of confetti for every visit.",
    layers: [
      { shape: "confetti", motion: "burst", phase: "intro", count: 24, from: "top", size: [14, 20], duration: [1600, 2400], delay: [0, 200], colors: CONFETTI, spin: 720, sway: 300, flip: true },
      { shape: "confetti", motion: "fall", phase: "idle", count: 8, from: "top", size: [12, 17], duration: [6000, 9000], colors: CONFETTI, spin: 540, sway: 20, flip: true, opacity: 0.9 },
    ],
  },
];

export const builtinEffect = (id: string | undefined): ProfileEffectSpec | undefined => (id ? BUILTIN_EFFECTS.find((e) => e.id === id) : undefined);

/** Lowercase letters, digits and dashes, up to 32: what the server takes. */
export const isEffectId = (id: string) => id.length <= 32 && /^[a-z0-9][a-z0-9-]*$/.test(id);

// ───────────────────────── Checking a spec ─────────────────────────

const oneOf = <T extends string>(value: unknown, options: readonly T[]): T | null => (options.includes(value as T) ? (value as T) : null);
const num = (value: unknown, min: number, max: number) => (typeof value === "number" && Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : null);
const range = (value: unknown, min: number, max: number): Range | null => {
  if (!Array.isArray(value) || value.length !== 2) return null;
  const a = num(value[0], min, max);
  const b = num(value[1], min, max);
  return a === null || b === null ? null : [Math.min(a, b), Math.max(a, b)];
};
const paint = (value: unknown): Paint | null =>
  typeof value === "string" && (/^#[0-9a-f]{6}$/i.test(value) || PAINT_TOKENS.includes(value as never)) ? (value as Paint) : null;

/**
 * A spec from anywhere (an instance's or a server's profile item, a file
 * someone made), made safe to play: only known shapes, motions, regions and
 * colors, and every number held to a range, so no spec can ask for
 * thousands of particles or minute-long frames. Null when there's nothing
 * left to play.
 *
 * Every spec that didn't come with the app goes through this before
 * `planEffect` (`lib/profile-items.ts`), and the cap on colors stays ahead
 * of the mapping.
 */
export function sanitizeEffect(raw: unknown): ProfileEffectSpec | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.id !== "string" || !isEffectId(r.id)) return null;
  const layers = (Array.isArray(r.layers) ? r.layers : []).slice(0, MAX_LAYERS).flatMap((l): EffectLayer[] => {
    if (!l || typeof l !== "object") return [];
    const x = l as Record<string, unknown>;
    const shape = oneOf(x.shape, SHAPES);
    const motion = oneOf(x.motion, MOTIONS);
    const phase = oneOf(x.phase, ["intro", "idle"] as const);
    const from = oneOf(x.from, REGIONS);
    const count = num(x.count, 0, MAX_PER_LAYER);
    const size = range(x.size, 2, 96);
    const duration = range(x.duration, 300, 20000);
    const colors = (Array.isArray(x.colors) ? x.colors.slice(0, 32) : []).map(paint).filter((c): c is Paint => !!c).slice(0, 8);
    if (!shape || !motion || !phase || !from || !count || !size || !duration || !colors.length) return [];
    const layer: EffectLayer = { shape, motion, phase, count: Math.round(count), from, size, duration, colors };
    const delay = range(x.delay, 0, 3000);
    if (delay) layer.delay = delay;
    const spin = num(x.spin, -1440, 1440);
    if (spin !== null) layer.spin = spin;
    const sway = num(x.sway, 0, 400);
    if (sway !== null) layer.sway = sway;
    const opacity = num(x.opacity, 0.05, 1);
    if (opacity !== null) layer.opacity = opacity;
    if (x.flip === true) layer.flip = true;
    if (x.glow === true) layer.glow = true;
    return [layer];
  });
  if (!layers.length) return null;
  const text = (value: unknown, max: number) => (typeof value === "string" ? value.trim().slice(0, max) : "");
  return { id: r.id, name: text(r.name, 40) || r.id, description: text(r.description, 120), layers };
}

// ───────────────────────── Planning particles ─────────────────────────

/** One particle, ready to play: its shape, look, and the keyframes it moves through. */
export type Particle = {
  shape: Shape;
  size: number;
  /** A CSS color. */
  color: string;
  glow: boolean;
  phase: "intro" | "idle";
  /** Only transform and opacity, so the compositor can run them. */
  keyframes: { transform: string; opacity: number; offset: number }[];
  duration: number;
  delay: number;
  /**
   * Where in its loop an idle particle starts (0 to 1), so the loop is
   * already in full swing when it fades in, rather than trickling in.
   */
  start: number;
  /** 1 for intros, Infinity for idle loops. */
  iterations: number;
  easing: string;
  /** Where it sits when nothing may move (reduced motion): a calm, readable still. */
  still: { transform: string; opacity: number } | null;
};

export type PlanOptions = {
  width: number;
  height: number;
  /** Mixed into the randomness, so each person's effect lands a little differently. */
  seed: string;
  /** Fewer particles and no glow, for devices that struggled. */
  lite?: boolean;
};

/** FNV-1a, for seeding. */
function hash(text: string) {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** mulberry32: small, fast, and the same sequence for the same seed everywhere. */
export function random(seed: string) {
  let a = hash(seed);
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const round = (n: number) => Math.round(n * 10) / 10;

export const paintCss = (p: Paint) => (p.startsWith("#") ? p : p === "profile" ? "var(--fx-profile, var(--primary))" : `var(--${p})`);

const transform = (x: number, y: number, rotate = 0, scale = 1, flip?: number) =>
  `translate3d(${round(x)}px, ${round(y)}px, 0) rotate(${round(rotate)}deg)${flip === undefined ? "" : ` rotateX(${round(flip)}deg)`} scale(${round(scale * 100) / 100})`;

/** Turns a spec into particles for a card of this size. */
export function planEffect(spec: ProfileEffectSpec, { width, height, seed, lite = false }: PlanOptions): Particle[] {
  const W = Math.max(40, width);
  const H = Math.max(40, height);
  // Counts scale with the card's area, within reason: a tile gets a handful, a big card a few more.
  const area = Math.min(1.4, Math.max(0.35, Math.sqrt((W * H) / (REFERENCE.width * REFERENCE.height))));
  // Distances (sway, flights) scale with the card's size.
  const reach = Math.min(1.4, Math.max(0.35, Math.min(W / REFERENCE.width, H / REFERENCE.height)));
  const particles: Particle[] = [];

  spec.layers.forEach((layer, n) => {
    const rnd = random(`${seed}:${spec.id}:${n}`);
    const between = ([a, b]: Range) => a + (b - a) * rnd();
    const pick = <T>(list: T[]) => list[Math.floor(rnd() * list.length)]!;
    const count = Math.min(MAX_PER_LAYER, Math.round(layer.count * area * (lite ? 0.5 : 1)));
    const peak = layer.opacity ?? 1;
    for (let i = 0; i < count && particles.length < MAX_PARTICLES; i++) {
      // Full size on a card, smaller only on tiles and other little previews.
      const size = round(between(layer.size) * Math.min(1, Math.max(0.55, W / 240)));
      const half = size / 2;
      const [x0, y0] = start(layer.from, W, H, size, rnd);
      const spin = (layer.spin ?? 0) * (rnd() < 0.5 ? -1 : 1) * (0.5 + rnd() * 0.5);
      const r0 = rnd() * 360 * (layer.spin ? 1 : 0);
      const sway = (layer.sway ?? 0) * reach;
      const flips = layer.flip ? 1 + Math.floor(rnd() * 3) : undefined;
      const flipAt = (k: number) => (flips === undefined ? undefined : k * flips * 360);
      const frames: Particle["keyframes"] = [];
      const frame = (offset: number, x: number, y: number, rotate: number, scale: number, opacity: number) =>
        frames.push({ offset, transform: transform(x - half, y - half, rotate, scale, flipAt(offset)), opacity: round(opacity * 100) / 100 });
      let easing = "linear";
      let still: Particle["still"] = null;

      switch (layer.motion) {
        case "fall":
        case "rise": {
          const down = layer.motion === "fall";
          const top = -size;
          const bottom = H + size;
          const phaseShift = rnd() * Math.PI * 2;
          for (let k = 0; k <= 4; k++) {
            const t = k / 4;
            const y = down ? top + (bottom - top) * t : bottom - (bottom - top) * t;
            const x = x0 + Math.sin(phaseShift + t * Math.PI * 2) * sway;
            frame(t, x, y, r0 + spin * t, 1, k === 0 || k === 4 ? 0 : k === 3 ? peak * 0.8 : peak);
          }
          // A still keeps to the edges so it never covers text.
          const sy = down ? between([0.02, 0.18]) * H : between([0.82, 0.98]) * H;
          still = { transform: transform(x0 - half, sy - half, r0, 1, flipAt(0.1)), opacity: peak * 0.8 };
          break;
        }
        case "twinkle": {
          if (layer.phase === "intro") {
            // Blinks on with a little overshoot, holds, then fades.
            frame(0, x0, y0, r0, 0, 0);
            frame(0.18, x0, y0, r0 + spin * 0.3, 1.15, peak);
            frame(0.3, x0, y0, r0 + spin * 0.4, 1, peak);
            frame(0.75, x0, y0, r0 + spin * 0.8, 1, peak * 0.9);
            frame(1, x0, y0, r0 + spin, 0, 0);
          } else {
            // A short glint, then a rest for the rest of the loop: idle stays calm.
            const glint = between([0.3, 0.5]);
            frame(0, x0, y0, r0, 0, 0);
            frame(glint * 0.45, x0, y0, r0 + spin * 0.5, 1, peak);
            frame(glint, x0, y0, r0 + spin, 0, 0);
            frame(1, x0, y0, r0 + spin, 0, 0);
          }
          easing = "ease-in-out";
          still = { transform: transform(x0 - half, y0 - half, r0, 0.85), opacity: peak * 0.75 };
          break;
        }
        case "drift": {
          const points = [0, 1, 2, 3].map(() => [x0 + (rnd() * 2 - 1) * sway, y0 + (rnd() * 2 - 1) * sway] as const);
          const glow = [0.25, 1, 0.4, 0.9];
          points.forEach(([x, y], k) => frame(k / 4, x, y, 0, k % 2 ? 1 : 0.8, peak * glow[k]!));
          frame(1, points[0]![0], points[0]![1], 0, 0.8, peak * glow[0]!);
          easing = "ease-in-out";
          still = { transform: transform(x0 - half, y0 - half), opacity: peak * 0.7 };
          break;
        }
        case "burst": {
          // Out from the origin in a cone pointed into the card, then falling away.
          const aim = Math.atan2(H / 2 - y0, W / 2 - x0);
          const angle = aim + (rnd() * 2 - 1) * (Math.PI / 2.6);
          const dist = sway * (0.45 + rnd() * 0.55);
          const x1 = x0 + Math.cos(angle) * dist;
          const y1 = y0 + Math.sin(angle) * dist;
          frame(0, x0, y0, r0, 0.3, 0);
          frame(0.08, x0 + (x1 - x0) * 0.2, y0 + (y1 - y0) * 0.2, r0 + spin * 0.15, 1, peak);
          frame(0.55, x1, y1, r0 + spin * 0.6, 1, peak);
          frame(1, x1 + Math.cos(angle) * dist * 0.15, y1 + H * 0.16, r0 + spin, 0.85, 0);
          easing = "cubic-bezier(0.16, 1, 0.3, 1)";
          break;
        }
        case "shoot": {
          // Down and to the left, across the top of the card, head first.
          const angle = Math.PI * (0.8 + rnd() * 0.08);
          const dist = W * 0.8;
          const dx = Math.cos(angle) * dist;
          const dy = Math.sin(angle) * dist;
          const deg = (angle * 180) / Math.PI;
          const sx = W * (0.55 + rnd() * 0.4);
          const sy = H * (0.02 + rnd() * 0.12);
          // Intros are one shot; idle ones streak for a fifth of the loop and rest.
          const span = layer.phase === "intro" ? 1 : 0.2;
          const streak = (t: number, scale: number, opacity: number) =>
            frames.push({
              offset: t,
              transform: `translate3d(${round(sx + (dx * t) / span - half)}px, ${round(sy + (dy * t) / span - half)}px, 0) rotate(${round(deg)}deg) scaleX(${scale})`,
              opacity: round(opacity * 100) / 100,
            });
          streak(0, 0.2, 0);
          streak(span * 0.25, 1, peak);
          streak(span, 0.4, 0);
          if (span < 1) frames.push({ ...frames[frames.length - 1]!, offset: 1 });
          easing = "ease-out";
          break;
        }
        case "pop": {
          frame(0, x0, y0, r0, 0, 0);
          frame(0.3, x0, y0 - size * 0.4, r0 + spin * 0.4, 1.1, peak);
          frame(0.8, x0, y0 - size * 0.8, r0 + spin * 0.8, 1, peak);
          frame(1, x0, y0 - size, r0 + spin, 1.3, 0);
          easing = "cubic-bezier(0.34, 1.56, 0.64, 1)";
          break;
        }
      }

      const intro = layer.phase === "intro";
      const duration = Math.round(between(layer.duration));
      particles.push({
        shape: layer.shape,
        size,
        color: paintCss(pick(layer.colors)),
        glow: !!layer.glow && !lite,
        phase: layer.phase,
        keyframes: frames,
        duration,
        delay: Math.round(intro ? between(layer.delay ?? [0, 0]) : 0),
        start: intro ? 0 : round(rnd() * 100) / 100,
        iterations: intro ? 1 : Infinity,
        easing,
        still,
      });
    }
  });
  return particles;
}

/** Where a particle starts, in pixels from the card's top left. */
function start(from: Region, W: number, H: number, size: number, rnd: () => number): [number, number] {
  const band = 0.16;
  switch (from) {
    case "top":
      return [rnd() * W, -size];
    case "bottom":
      return [W * (0.2 + rnd() * 0.6), H + size * 0.5];
    case "top-left":
      return [-size + rnd() * W * 0.12, -size + rnd() * H * 0.08];
    case "top-right":
      return [W + size - rnd() * W * 0.12, -size + rnd() * H * 0.08];
    case "center":
      return [W * (0.4 + rnd() * 0.2), H * (0.4 + rnd() * 0.2)];
    case "corners": {
      const right = rnd() < 0.5;
      const low = rnd() < 0.5;
      return [right ? W * (1 - rnd() * 0.22) : W * rnd() * 0.22, low ? H * (1 - rnd() * 0.18) : H * rnd() * 0.18];
    }
    case "edges": {
      // A band around the card, weighted by its length, so text in the middle stays clear.
      const side = rnd();
      if (side < 0.3) return [rnd() * W, rnd() * H * band];
      if (side < 0.45) return [rnd() * W, H * (1 - rnd() * band * 0.7)];
      if (side < 0.725) return [rnd() * W * band, rnd() * H];
      return [W * (1 - rnd() * band), rnd() * H];
    }
    case "anywhere":
      return [rnd() * W, rnd() * H];
  }
}

/** When idle particles fade in: as the intro winds down, so the two overlap. */
export const idleFadeAt = (particles: Particle[]) => Math.round(introLength(particles) * 0.45);
export const IDLE_FADE_MS = 900;

/** How long the intro runs, in milliseconds. */
export const introLength = (particles: Particle[]) =>
  Math.max(0, ...particles.filter((p) => p.phase === "intro").map((p) => p.delay + p.duration));

// ───────────────────────── Shapes ─────────────────────────

/** SVG for each shape, in a 24 by 24 box. Filled ones use the particle's color; outlined ones stroke it. */
export const SHAPE_PATHS: Record<Shape, { d: string; stroke?: number }> = {
  petal: { d: "M12 22c-3.6-2.5-6.8-6.4-6.2-10.6C6.4 7.2 9.4 4.4 11 2.4l1 2 1-2c1.6 2 4.6 4.8 5.2 9C18.8 15.6 15.6 19.5 12 22Z" },
  star: { d: "M12 2.5l2.8 6 6.5.7-4.9 4.4 1.4 6.4L12 16.7 6.2 20l1.4-6.4-4.9-4.4 6.5-.7z" },
  sparkle: { d: "M12 1c.9 6.2 3.8 9.1 10 10-6.2.9-9.1 3.8-10 10-.9-6.2-3.8-9.1-10-10 6.2-.9 9.1-3.8 10-10Z" },
  heart: { d: "M12 21s-7.6-4.7-9.7-9.3C.8 8.3 3 4.5 6.7 4.5c2.1 0 3.5 1.1 5.3 3 1.8-1.9 3.2-3 5.3-3 3.7 0 5.9 3.8 4.4 7.2C19.6 16.3 12 21 12 21Z" },
  snowflake: { d: "M12 2v20M3.3 7l17.4 10M3.3 17L20.7 7M12 2l-2.5 2.5M12 2l2.5 2.5M12 22l-2.5-2.5M12 22l2.5-2.5", stroke: 1.8 },
  bubble: { d: "M12 2.5a9.5 9.5 0 1 0 0 19 9.5 9.5 0 1 0 0-19ZM7.5 9.5a4.5 4.5 0 0 1 3-3", stroke: 1.4 },
  dot: { d: "M12 4a8 8 0 1 0 0 16 8 8 0 1 0 0-16Z" },
  confetti: { d: "M8 3h8v18H8z" },
  streak: { d: "M0 11.4h20.5l3.5.6-3.5.6H0z" },
};
