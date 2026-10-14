/**
 * The color side of the Accessibility settings (docs/themes.md,
 * "Accessibility"): correcting colors for color blindness, and the high
 * contrast version of any theme. Pure, and the desktop's
 * `core/accessibility.rs` does the same math, so both apps look alike.
 */

import { mix, type ThemeTokens } from "./themes.ts";

export type ColorVision = "off" | "protanopia" | "deuteranopia" | "tritanopia";
export const COLOR_VISIONS: readonly ColorVision[] = ["off", "protanopia", "deuteranopia", "tritanopia"];

/** A 3×3 matrix over sRGB channels (0 to 1), row by row. */
export type Matrix = readonly number[];

const IDENTITY: Matrix = [1, 0, 0, 0, 1, 0, 0, 0, 1];

/** How each kind of color blindness sees sRGB (Machado, Oliveira and Fernandes 2009, severity 1). */
const SEEN: Record<Exclude<ColorVision, "off">, Matrix> = {
  protanopia: [0.152286, 1.052583, -0.204868, 0.114503, 0.786281, 0.099216, -0.003882, -0.048116, 1.051998],
  deuteranopia: [0.367322, 0.860646, -0.227968, 0.280085, 0.672501, 0.047413, -0.01182, 0.04294, 0.968881],
  tritanopia: [1.255528, -0.076749, -0.178779, -0.078411, 0.930809, 0.147602, 0.004733, 0.691367, 0.3039],
};

/** Where the difference someone can't see goes: red-green into green and blue, blue-yellow into red and green. */
const SHIFT: Record<Exclude<ColorVision, "off">, Matrix> = {
  protanopia: [0, 0, 0, 0.7, 1, 0, 0.7, 0, 1],
  deuteranopia: [0, 0, 0, 0.7, 1, 0, 0.7, 0, 1],
  tritanopia: [1, 0, 0.7, 0, 1, 0.7, 0, 0, 0],
};

function multiply(a: Matrix, b: Matrix): number[] {
  const out: number[] = [];
  for (let r = 0; r < 3; r++) for (let c = 0; c < 3; c++) out.push(a[r * 3]! * b[c]! + a[r * 3 + 1]! * b[3 + c]! + a[r * 3 + 2]! * b[6 + c]!);
  return out;
}

/** How someone with this kind of color blindness sees colors (for checks and tests). */
export const seenBy = (kind: ColorVision): Matrix => (kind === "off" ? IDENTITY : SEEN[kind]);

/**
 * The correction (daltonizing): each color plus the part of it this kind of
 * color blindness loses, moved where it can be seen, `strength` (0 to 1) of
 * the way. `I + strength · SHIFT · (I − SEEN)`.
 */
export function colorVisionMatrix(kind: ColorVision, strength = 1): Matrix {
  if (kind === "off" || strength <= 0) return IDENTITY;
  const lost = IDENTITY.map((v, i) => v - SEEN[kind][i]!);
  const moved = multiply(SHIFT[kind], lost);
  return IDENTITY.map((v, i) => v + strength * moved[i]!);
}

/** The values of an SVG `feColorMatrix` (4×5, alpha kept) for a 3×3 matrix. */
export function feColorMatrix(m: Matrix): string {
  const row = (r: number) => [m[r * 3]!, m[r * 3 + 1]!, m[r * 3 + 2]!, 0, 0].map((v) => +v.toFixed(6)).join(" ");
  return [row(0), row(1), row(2), "0 0 0 1 0"].join(" ");
}

const channels = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
const toHex = (c: number[]) => `#${c.map((v) => Math.round(Math.min(1, Math.max(0, v)) * 255).toString(16).padStart(2, "0")).join("")}`;

/** A #rrggbb color through a matrix, clamped. */
export function applyMatrix(m: Matrix, hex: string): string {
  const [r, g, b] = channels(hex) as [number, number, number];
  return toHex([0, 1, 2].map((i) => m[i * 3]! * r + m[i * 3 + 1]! * g + m[i * 3 + 2]! * b));
}

// ───────────────────────── High contrast ─────────────────────────

/** WCAG relative luminance of a #rrggbb color. */
export function luminance(hex: string): number {
  const [r, g, b] = channels(hex).map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio, 1 to 21. */
export function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/**
 * The color as near as it can stay to itself with at least `ratio` against
 * every color in `against`: mixed toward black or white (whichever stands
 * out more), as little as it takes. If even that can't, black or white.
 */
export function ensureContrast(color: string, against: string[], ratio: number): string {
  const worst = (c: string) => Math.min(...against.map((a) => contrast(c, a)));
  if (worst(color) >= ratio) return color;
  const target = worst("#000000") >= worst("#ffffff") ? "#000000" : "#ffffff";
  if (worst(target) < ratio) return target;
  let [lo, hi] = [0, 1];
  for (let i = 0; i < 16; i++) {
    const mid = (lo + hi) / 2;
    if (worst(mix(target, color, mid)) >= ratio) hi = mid;
    else lo = mid;
  }
  return mix(target, color, hi);
}

/**
 * A theme's tokens in high contrast: surfaces halfway to black (dark themes)
 * or white (light ones), text at 12:1 against them, quiet text at 7:1,
 * buttons, links and danger at 4.5:1, and lines and focus rings at 3:1.
 */
export function highContrastTokens(t: ThemeTokens): ThemeTokens {
  const dark = luminance(t.background) < luminance(t.foreground);
  const deep = (c: string) => mix(dark ? "#000000" : "#ffffff", c, 0.5);
  const background = deep(t.background);
  const card = deep(t.card);
  const popover = deep(t.popover);
  const surfaces = [background, card];
  const accent = ensureContrast(t.accent, [background], 1.5);
  const primary = ensureContrast(t.primary, surfaces, 4.5);
  return {
    background,
    foreground: ensureContrast(t.foreground, surfaces, 12),
    card,
    "card-foreground": ensureContrast(t["card-foreground"], [card], 12),
    popover,
    "popover-foreground": ensureContrast(t["popover-foreground"], [popover], 12),
    primary,
    "primary-foreground": ensureContrast(t["primary-foreground"], [primary], 7),
    secondary: t.secondary,
    "secondary-foreground": ensureContrast(t["secondary-foreground"], [t.secondary], 7),
    muted: t.muted,
    "muted-foreground": ensureContrast(t["muted-foreground"], [...surfaces, t.muted], 7),
    accent,
    "accent-foreground": ensureContrast(t["accent-foreground"], [accent], 7),
    destructive: ensureContrast(t.destructive, surfaces, 4.5),
    border: ensureContrast(t.border, surfaces, 3),
    input: ensureContrast(t.input, surfaces, 3),
    ring: ensureContrast(t.ring, surfaces, 3),
  };
}

// ───────────────────────── Encryption marks ─────────────────────────

/** What end-to-end encryption is drawn with: green, and amber when a safety number changed. */
export type SealColors = {
  /** A pill or note's own solid surface, and pointed at. */
  surface: string;
  surfaceHover: string;
  /** Text and icons on that surface. */
  ink: string;
  /** Its outline, so it stands apart from whatever is behind it. */
  edge: string;
  /** A mark on its own, on the theme's card or background. */
  icon: string;
  warnSurface: string;
  warnSurfaceHover: string;
  warnInk: string;
  warnEdge: string;
  warnIcon: string;
};

const SEAL_GREEN = "#00bc7d";
const SEAL_AMBER = "#fe9a00";

/**
 * The encryption marks in a theme. They sit over see-through panels and
 * backdrop pictures, so each draws on a solid surface of its own (the
 * theme's card, tinted), never on what's behind it, with text at 4.5:1
 * against that surface and lone icons at 3:1 against the card and the
 * background, whatever the theme. The desktop's `core/accessibility.rs`
 * does the same math.
 */
export function sealColors(t: ThemeTokens): SealColors {
  const dark = luminance(t.card) < luminance(t["card-foreground"]);
  const paint = (hue: string, ink: string, icon: string) => {
    const surface = mix(hue, t.card, 0.14);
    const surfaceHover = mix(hue, t.card, 0.22);
    return {
      surface,
      surfaceHover,
      ink: ensureContrast(ink, [surface, surfaceHover], 4.5),
      edge: ensureContrast(mix(hue, t.card, 0.45), [surface], 2),
      icon: ensureContrast(icon, [t.card, t.background], 3),
    };
  };
  const green = paint(SEAL_GREEN, dark ? "#5ee9b5" : "#007a55", dark ? "#00d492" : "#009966");
  const amber = paint(SEAL_AMBER, dark ? "#ffd230" : "#bb4d00", dark ? "#ffb900" : "#e17100");
  return {
    ...green,
    warnSurface: amber.surface,
    warnSurfaceHover: amber.surfaceHover,
    warnInk: amber.ink,
    warnEdge: amber.edge,
    warnIcon: amber.icon,
  };
}
