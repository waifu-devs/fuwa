/**
 * Themes are shadcn/ui theme variants: every theme, built-in or made by a
 * member, is a full set of values for the same shadcn CSS variables
 * (--background, --primary, --radius, ...). Components only ever reference
 * those variables, so any theme restyles every component.
 */

import type { Key } from "@/i18n/i18n";

export const TOKENS = [
  "background",
  "foreground",
  "card",
  "card-foreground",
  "popover",
  "popover-foreground",
  "primary",
  "primary-foreground",
  "secondary",
  "secondary-foreground",
  "muted",
  "muted-foreground",
  "accent",
  "accent-foreground",
  "destructive",
  "border",
  "input",
  "ring",
] as const;
export type Token = (typeof TOKENS)[number];
export type ThemeTokens = Record<Token, string>;

export type ThemeVariant = { tokens: ThemeTokens; radius: number };

export type Theme = {
  id: string;
  name: string;
  description: string | null;
  variant: ThemeVariant;
  builtin: boolean;
  isPublic?: boolean;
  ownerUsername?: string;
};

/** The handful of colors the quick editor exposes; everything else is derived from them. */
export const SEEDS = ["background", "foreground", "card", "primary", "primary-foreground", "muted-foreground", "border"] as const;
export type Seed = (typeof SEEDS)[number];
export type ThemeSeeds = Record<Seed, string>;

/** Each color's name in the theme editor, as catalog keys (locales/, appsettings.themes.token.*). */
export const TOKEN_LABELS: Record<Token, Key> = {
  background: "appsettings.themes.token.background",
  foreground: "appsettings.themes.token.foreground",
  card: "appsettings.themes.token.card",
  "card-foreground": "appsettings.themes.token.cardForeground",
  popover: "appsettings.themes.token.popover",
  "popover-foreground": "appsettings.themes.token.popoverForeground",
  primary: "appsettings.themes.token.primary",
  "primary-foreground": "appsettings.themes.token.primaryForeground",
  secondary: "appsettings.themes.token.secondary",
  "secondary-foreground": "appsettings.themes.token.secondaryForeground",
  muted: "appsettings.themes.token.muted",
  "muted-foreground": "appsettings.themes.token.mutedForeground",
  accent: "appsettings.themes.token.accent",
  "accent-foreground": "appsettings.themes.token.accentForeground",
  destructive: "appsettings.themes.token.destructive",
  border: "appsettings.themes.token.border",
  input: "appsettings.themes.token.input",
  ring: "appsettings.themes.token.ring",
};

/** A #rrggbb color. */
export const HEX = /^#[0-9a-f]{6}$/i;

/** Mixes `a` into `b` by `amount` (0..1) in sRGB. */
export function mix(a: string, b: string, amount: number): string {
  const pa = [1, 3, 5].map((i) => parseInt(a.slice(i, i + 2), 16));
  const pb = [1, 3, 5].map((i) => parseInt(b.slice(i, i + 2), 16));
  return `#${pa.map((v, i) => Math.round(v * amount + pb[i] * (1 - amount)).toString(16).padStart(2, "0")).join("")}`;
}

/** Expands the quick-editor seeds into a full shadcn token set. */
export function deriveTokens(s: ThemeSeeds): ThemeTokens {
  return {
    background: s.background,
    foreground: s.foreground,
    card: s.card,
    "card-foreground": s.foreground,
    popover: s.card,
    "popover-foreground": s.foreground,
    primary: s.primary,
    "primary-foreground": s["primary-foreground"],
    secondary: mix(s.primary, s.card, 0.12),
    "secondary-foreground": s.foreground,
    muted: mix(s.border, s.background, 0.45),
    "muted-foreground": s["muted-foreground"],
    accent: mix(s.primary, s.background, 0.14),
    "accent-foreground": s.foreground,
    destructive: "#e5484d",
    border: s.border,
    input: s.border,
    ring: s.primary,
  };
}

export function seedsOf(tokens: ThemeTokens): ThemeSeeds {
  return Object.fromEntries(SEEDS.map((k) => [k, tokens[k]])) as ThemeSeeds;
}

/** A built-in theme. Its line is in the catalog (appsettings.themes.builtin.*), shown by the theme picker. */
function builtin(id: string, name: string, radius: number, seeds: ThemeSeeds): Theme {
  return { id, name, description: null, builtin: true, variant: { tokens: deriveTokens(seeds), radius } };
}

export const BUILTIN_THEMES: Theme[] = [
  builtin("sakura", "Sakura", 1, {
    background: "#fff5f8",
    foreground: "#3b2330",
    card: "#ffffff",
    primary: "#f06292",
    "primary-foreground": "#ffffff",
    "muted-foreground": "#8a6577",
    border: "#f8d3e0",
  }),
  builtin("yoru", "Yoru", 0.75, {
    background: "#14111f",
    foreground: "#ece6ff",
    card: "#1f1a2e",
    primary: "#b388ff",
    "primary-foreground": "#14111f",
    "muted-foreground": "#9a90b8",
    border: "#342b4d",
  }),
  builtin("matcha", "Matcha", 0.5, {
    background: "#f4f6ec",
    foreground: "#243021",
    card: "#fffef7",
    primary: "#5a8a3c",
    "primary-foreground": "#ffffff",
    "muted-foreground": "#66735f",
    border: "#d9e2c8",
  }),
  builtin("sora", "Sora", 1.25, {
    background: "#f0f7ff",
    foreground: "#1a2b44",
    card: "#ffffff",
    primary: "#3b8beb",
    "primary-foreground": "#ffffff",
    "muted-foreground": "#5f7391",
    border: "#cfe2f7",
  }),
  builtin("tsundere", "Tsundere", 0.25, {
    background: "#1a0f12",
    foreground: "#ffe9ec",
    card: "#2a171c",
    primary: "#ff4d6d",
    "primary-foreground": "#1a0f12",
    "muted-foreground": "#c28c96",
    border: "#4a2630",
  }),
];

export const DEFAULT_THEME = BUILTIN_THEMES[0];

export const RADIUS_MIN = 0;
export const RADIUS_MAX = 1.5;

/** The inline style that applies a theme variant to an element and everything inside it. */
export function themeStyle(variant: ThemeVariant): Record<string, string> {
  const style: Record<string, string> = { "--radius": `${variant.radius}rem` };
  for (const key of TOKENS) style[`--${key}`] = variant.tokens[key];
  return style;
}

// ───────────────────────── fuwa ─────────────────────────
// The themes above are the waifu.dev site's built-ins, copied as-is so fuwa
// looks like the rest of Waifu Devs. Which one is on screen is an app
// setting (lib/prefs.ts).

/** Paints the whole page in a theme. The tokens are registered properties, so the change cross-fades. */
export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  for (const [key, value] of Object.entries(themeStyle(theme.variant))) root.style.setProperty(key, value);
  const dark = isDark(theme);
  root.classList.toggle("dark", dark);
  root.style.colorScheme = dark ? "dark" : "light";
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", theme.variant.tokens.background);
}

/** Dark when the background is darker than mid-grey. */
export function isDark(theme: Theme): boolean {
  const hex = theme.variant.tokens.background;
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
  return 0.2126 * r! + 0.7152 * g! + 0.0722 * b! < 128;
}
