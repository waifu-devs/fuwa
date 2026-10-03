/**
 * What sits behind the app: a background picture and an effect. Both are
 * part of the fuwa theme format (lib/theme-file.ts), and also an app setting
 * of their own that applies under any theme without one.
 *
 * Pictures only ever come from fuwa instances: someone uploads one to an
 * instance they're signed in to (MediaService's backgrounds), and the link is
 * shown only while that instance is one the app trusts (lib/shown.ts).
 */

/** Effects drawn on the GPU (with a CSS stand-in where WebGPU isn't available). */
export const SHADER_EFFECTS = ["aurora", "petals", "stars", "waves"] as const;
/** Still textures, plain CSS. */
export const TEXTURE_EFFECTS = ["grain", "paper", "dots", "grid"] as const;
export const EFFECTS = ["none", ...SHADER_EFFECTS, ...TEXTURE_EFFECTS] as const;
export type Effect = (typeof EFFECTS)[number];
export type ShaderEffect = (typeof SHADER_EFFECTS)[number];

export const isShader = (effect: Effect): effect is ShaderEffect => (SHADER_EFFECTS as readonly string[]).includes(effect);

export const EFFECT_INFO: Record<Effect, { name: string; hint: string }> = {
  none: { name: "None", hint: "Just the theme." },
  aurora: { name: "Aurora", hint: "Slow ribbons of the theme's colors." },
  petals: { name: "Petals", hint: "Blossoms drifting down." },
  stars: { name: "Starfield", hint: "Twinkling stars, gently drifting." },
  waves: { name: "Waves", hint: "Soft layered waves rolling by." },
  grain: { name: "Film grain", hint: "A fine, still noise." },
  paper: { name: "Paper", hint: "Warm fibers like washi paper." },
  dots: { name: "Dots", hint: "A tidy dot pattern." },
  grid: { name: "Grid", hint: "Notebook grid lines." },
};

export type Fit = "cover" | "contain" | "tile";

export type Backdrop = {
  /** A background picture on a fuwa instance, or "" for none. */
  image: string;
  fit: Fit;
  /** How much the theme's background covers the picture, in percent. */
  dim: number;
  /** Picture blur, in pixels. */
  blur: number;
  effect: Effect;
  /** How strong the effect is, in percent. */
  intensity: number;
  /** How fast the effect moves, in percent of normal. */
  speed: number;
  /** How solid the chat is over a backdrop, in percent (the sidebars are 20 points more). */
  panels: number;
};

export const DEFAULT_BACKDROP: Backdrop = {
  image: "",
  fit: "cover",
  dim: 35,
  blur: 0,
  effect: "none",
  intensity: 70,
  speed: 100,
  panels: 35,
};

export const LIMITS = {
  dim: [0, 90],
  blur: [0, 24],
  intensity: [0, 100],
  speed: [0, 200],
  panels: [20, 100],
} as const satisfies Record<string, readonly [number, number]>;

const clamp = (n: unknown, [min, max]: readonly [number, number], fallback: number) =>
  typeof n === "number" && Number.isFinite(n) ? Math.min(max, Math.max(min, Math.round(n))) : fallback;

/** An https (or local http) link to a picture at /media/<id> on some instance. */
export function isMediaLink(url: unknown): url is string {
  if (typeof url !== "string" || url.length > 512) return false;
  try {
    const u = new URL(url);
    return (u.protocol === "https:" || u.protocol === "http:") && /^\/media\/[0-9A-Za-z]{10,40}$/.test(u.pathname) && !u.search && !u.hash && !u.username;
  } catch {
    return false;
  }
}

/** Anything stored or imported, made into a backdrop that can't break the app or load from anywhere but an instance. */
export function sanitizeBackdrop(value: unknown): Backdrop {
  const b = (value && typeof value === "object" ? value : {}) as Partial<Record<keyof Backdrop, unknown>>;
  const d = DEFAULT_BACKDROP;
  return {
    image: isMediaLink(b.image) ? b.image : "",
    fit: b.fit === "contain" || b.fit === "tile" ? b.fit : "cover",
    dim: clamp(b.dim, LIMITS.dim, d.dim),
    blur: clamp(b.blur, LIMITS.blur, d.blur),
    effect: (EFFECTS as readonly unknown[]).includes(b.effect) ? (b.effect as Effect) : "none",
    intensity: clamp(b.intensity, LIMITS.intensity, d.intensity),
    speed: clamp(b.speed, LIMITS.speed, d.speed),
    panels: clamp(b.panels, LIMITS.panels, d.panels),
  };
}

/** Whether there's anything behind the app at all. */
export const hasBackdrop = (b: Backdrop) => !!b.image || b.effect !== "none";
