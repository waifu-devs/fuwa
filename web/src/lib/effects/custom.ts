import type { ShaderEffect } from "@/lib/backdrop";

/**
 * Custom shaders: effects people write themselves, in WGSL, kept in a
 * backdrop (and so in theme files). Someone writes one function,
 *
 *   fn shade(uv: vec2f) -> vec4f
 *
 * which returns a color and how much of it covers what's behind (straight
 * alpha, 0 to 1). fuwa puts it between its own prelude (the inputs, as the
 * uniform `fuwa`, and a few helpers) and its own entry point, which applies the
 * strength and premultiplies. The shader can't declare anything the app doesn't
 * give it: attributes (`@group`, `@binding`, entry points) are refused, so it
 * has no textures, no buffers and nothing from anywhere but these inputs.
 *
 * A shader that doesn't pass these checks, doesn't compile, is too slow for the
 * device or stops the GPU never breaks the app: the backdrop shows the
 * shader's fallback effect instead, and the editor says why.
 */

export type CustomShader = {
  /** What it's called on the effect card. */
  name: string;
  /** The WGSL, defining `fn shade(uv: vec2f) -> vec4f`. */
  code: string;
  /** What shows where it can't run: no WebGPU, or the shader has a problem. */
  fallback: ShaderEffect | "none";
};

/** At most this many bytes of WGSL (UTF-8). Plenty for an effect; small enough for any prefs store or theme file. */
export const MAX_SHADER_BYTES = 16 * 1024;
export const SHADER_NAME_MAX = 32;
export const FALLBACKS = ["none", "aurora", "petals", "stars", "waves"] as const;

const bytes = (s: string) => new TextEncoder().encode(s).length;

/** The inputs, as WGSL. Kept in sync with docs/themes.md. */
const INPUTS = /* wgsl */ `struct Fuwa {
  primary: vec4f,
  accent: vec4f,
  background: vec4f,
  foreground: vec4f,
  res: vec2f,
  pointer: vec2f,
  time: f32,
  intensity: f32,
}
@group(0) @binding(0) var<uniform> fuwa: Fuwa;

fn hash21(p: vec2f) -> f32 {
  var q = fract(p * vec2f(123.34, 456.21));
  q += dot(q, q + 45.32);
  return fract(q.x * q.y);
}

fn noise(p: vec2f) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let u = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash21(i), hash21(i + vec2f(1.0, 0.0)), u.x), mix(hash21(i + vec2f(0.0, 1.0)), hash21(i + vec2f(1.0, 1.0)), u.x), u.y);
}

fn fbm(p: vec2f) -> f32 {
  var v = 0.0;
  var a = 0.5;
  var q = p;
  for (var i = 0; i < 4; i++) {
    v += a * noise(q);
    q = q * 2.03 + vec2f(1.7, 9.2);
    a *= 0.5;
  }
  return v;
}
`;

const MAIN = /* wgsl */ `
@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
  let c = shade(uv);
  let a = clamp(c.a * fuwa.intensity, 0.0, 1.0);
  return vec4f(clamp(c.rgb, vec3f(0.0), vec3f(1.0)) * a, a);
}
`;

/** Lines before the shader's own first line in the full source, to point errors at the right line. */
export const PRELUDE_LINES = INPUTS.split("\n").length;

/** The whole WGSL module for a shader. */
export const fullSource = (code: string) => `${INPUTS}\n${code}\n${MAIN}`;

/** The shader's line for a line of the full source, or null when it's in fuwa's own part. */
export function shaderLine(full: number, code: string): number | null {
  const line = full - PRELUDE_LINES;
  return line >= 1 && line <= code.split("\n").length ? line : null;
}

const stripComments = (code: string) => code.replace(/\/\*[\s\S]*?(\*\/|$)/g, " ").replace(/\/\/[^\n]*/g, "");

/**
 * What's wrong with a shader before it goes near a GPU, or null. These are
 * the rules that keep it to the app's inputs; everything else is the
 * compiler's to say.
 */
export function shaderProblem(code: string): string | null {
  if (!code.trim()) return "The shader is empty.";
  if (bytes(code) > MAX_SHADER_BYTES) return `Shaders can be at most ${MAX_SHADER_BYTES / 1024} KB.`;
  const live = stripComments(code);
  if (/[a-z][a-z0-9+.-]*:\/\//i.test(live)) return "Shaders can't point at links. Everything they draw comes from the inputs fuwa gives them.";
  if (live.includes("@")) return "Shaders can't use attributes (@…): fuwa gives every input, so there's nothing to bind.";
  if (/^\s*(enable|requires|diagnostic)\b/m.test(live)) return "Shaders can't turn on extensions.";
  if (!/\bfn\s+shade\s*\(/.test(live)) return "Define fn shade(uv: vec2f) -> vec4f: it's what fuwa calls for every pixel.";
  return null;
}

/** A shader as stored or imported: text that can't break anything, its size capped. Its code may still have problems (shaderProblem). */
export function sanitizeShader(value: unknown): CustomShader | null {
  if (!value || typeof value !== "object") return null;
  const s = value as Record<string, unknown>;
  if (typeof s.code !== "string") return null;
  // Line ends made plain, and no control characters but tabs and line ends.
  let code = s.code.replace(/\r\n?/g, "\n").replace(/[\u0000-\u0008\u000b-\u001f\u007f]/g, "");
  while (bytes(code) > MAX_SHADER_BYTES) code = code.slice(0, code.length - Math.max(1, Math.ceil((bytes(code) - MAX_SHADER_BYTES) / 3)));
  const name = typeof s.name === "string" ? s.name.replace(/[\u0000-\u001f\u007f]/g, " ").trim().slice(0, SHADER_NAME_MAX) : "";
  return {
    name: name || "My shader",
    code,
    fallback: (FALLBACKS as readonly unknown[]).includes(s.fallback) ? (s.fallback as CustomShader["fallback"]) : "aurora",
  };
}

/** A short, stable id for a shader's code (FNV-1a), to remember what happened to it. */
export function shaderId(code: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < code.length; i++) {
    h ^= code.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(36);
}

// ───────────────────────── Starters ─────────────────────────

/** Shaders to start from, each a small lesson in the inputs. */
export const STARTERS: { id: string; name: string; hint: string; shader: CustomShader }[] = [
  {
    id: "glow",
    name: "Glow",
    hint: "A soft light that follows your pointer.",
    shader: {
      name: "Glow",
      fallback: "aurora",
      code: `// A soft light that follows the pointer, breathing slowly.
fn shade(uv: vec2f) -> vec4f {
  let aspect = fuwa.res.x / fuwa.res.y;
  let d = (uv - fuwa.pointer) * vec2f(aspect, 1.0);
  let breathe = 0.85 + 0.15 * sin(fuwa.time * 1.2);
  let glow = exp(-dot(d, d) * 9.0) * breathe;
  let color = mix(fuwa.accent.rgb, fuwa.primary.rgb, glow);
  return vec4f(color, glow * 0.8);
}
`,
    },
  },
  {
    id: "plasma",
    name: "Plasma",
    hint: "Melting colors, like a lava lamp.",
    shader: {
      name: "Plasma",
      fallback: "aurora",
      code: `// Melting color fields in the theme's primary and accent.
fn shade(uv: vec2f) -> vec4f {
  let p = uv * vec2f(fuwa.res.x / fuwa.res.y, 1.0) * 4.0;
  let t = fuwa.time * 0.3;
  let v = sin(p.x + t) + sin(p.y * 1.3 - t) + sin((p.x + p.y) * 0.7 + t * 1.7)
    + sin(length(p - vec2f(2.0, 1.5)) * 1.8 - t * 2.0);
  let k = 0.5 + 0.5 * sin(v * 1.4);
  let color = mix(fuwa.primary.rgb, fuwa.accent.rgb, k);
  return vec4f(color, 0.4 + 0.4 * k);
}
`,
    },
  },
  {
    id: "ripples",
    name: "Ripples",
    hint: "Rings spreading from the pointer.",
    shader: {
      name: "Ripples",
      fallback: "waves",
      code: `// Rings spreading out from the pointer, fading as they go.
fn shade(uv: vec2f) -> vec4f {
  let aspect = fuwa.res.x / fuwa.res.y;
  let d = length((uv - fuwa.pointer) * vec2f(aspect, 1.0));
  let rings = 0.5 + 0.5 * sin(d * 40.0 - fuwa.time * 3.0);
  let fade = exp(-d * 3.0);
  let line = smoothstep(0.85, 1.0, rings) * fade;
  return vec4f(mix(fuwa.primary.rgb, vec3f(1.0), 0.3), line);
}
`,
    },
  },
  {
    id: "clouds",
    name: "Clouds",
    hint: "Drifting clouds, with fbm noise.",
    shader: {
      name: "Clouds",
      fallback: "aurora",
      code: `// Clouds drifting by, made with fbm (layered noise).
fn shade(uv: vec2f) -> vec4f {
  let p = uv * vec2f(fuwa.res.x / fuwa.res.y, 1.0) * 2.5;
  let t = fuwa.time * 0.04;
  let n = fbm(p + vec2f(t * 3.0, t) + fbm(p * 1.5 - t) * 0.6);
  let cloud = smoothstep(0.42, 0.78, n);
  // Pale at the tops, the accent in the shade underneath.
  let color = mix(fuwa.accent.rgb, mix(fuwa.primary.rgb, vec3f(1.0), 0.7), cloud);
  return vec4f(color, cloud * 0.75);
}
`,
    },
  },
];

export const DEFAULT_SHADER: CustomShader = STARTERS[0]!.shader;
