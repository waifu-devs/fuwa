import type { I18n } from "@/i18n/i18n";
import { sanitizeBackdrop, type Backdrop } from "@/lib/backdrop";
import { shaderProblem } from "@/lib/effects/custom";
import { deriveTokens, HEX, RADIUS_MAX, RADIUS_MIN, SEEDS, TOKENS, type Theme, type ThemeSeeds, type ThemeTokens } from "@/lib/themes";

/**
 * Custom themes, and the fuwa theme file every fuwa app reads and writes
 * (docs/themes.md has the format). A custom theme is a built-in theme's shape
 * (the shadcn tokens and a radius) plus, optionally, its own backdrop.
 *
 * A theme file is data only: colors, numbers and words. Nothing in it can make
 * an app load anything from anywhere (a custom shader is WGSL that can only
 * read the inputs the app gives it). Its background picture travels inside
 * the file (a data: URL) and is uploaded to your instance on import; links in
 * a file are ignored.
 */

export type CustomTheme = Theme & {
  builtin: false;
  /** Its own backdrop, used while it's the theme on screen. Null keeps the app's backdrop setting. */
  backdrop: Backdrop | null;
  updatedAt: number;
};

/** What a theme file says it is. */
export const FORMAT = "fuwa-theme";
export const VERSION = 1;
/** Pictures in theme files: the types instances take, at most this big once decoded. */
export const FILE_PICTURE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/avif"];
export const MAX_FILE_PICTURE_BYTES = 12 * 1024 * 1024;
export const MAX_CUSTOM_THEMES = 50;
export const NAME_MAX = 40;
export const DESCRIPTION_MAX = 140;

/** The file as written. Keys are stable across apps; unknown ones are ignored on read. */
export type ThemeFile = {
  format: typeof FORMAT;
  version: number;
  name: string;
  description: string;
  colors: ThemeTokens;
  /** Corner radius, in rem. */
  radius: number;
  backdrop: (Omit<Backdrop, "image"> & { image: string | null }) | null;
};

/** A theme read from a file, and its picture (still to be uploaded) if it had one. */
export type Imported = { theme: CustomTheme; picture: Blob | null; notes: string[] };

export const newThemeId = () => `custom-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;

const text = (value: unknown, max: number) => (typeof value === "string" ? value.replace(/[\u0000-\u001f\u007f]/g, " ").trim().slice(0, max) : "");

const radius = (value: unknown, fallback = 0.75) =>
  typeof value === "number" && Number.isFinite(value) ? Math.min(RADIUS_MAX, Math.max(RADIUS_MIN, Math.round(value * 100) / 100)) : fallback;

/** A full token set from what a file or store holds: all of them, or at least the editor's seeds. */
function readColors(value: unknown): ThemeTokens | null {
  if (!value || typeof value !== "object") return null;
  const colors = value as Record<string, unknown>;
  const ok = (k: string) => typeof colors[k] === "string" && HEX.test(colors[k] as string);
  if (TOKENS.every(ok)) return Object.fromEntries(TOKENS.map((k) => [k, (colors[k] as string).toLowerCase()])) as ThemeTokens;
  if (!SEEDS.every(ok)) return null;
  const seeds = Object.fromEntries(SEEDS.map((k) => [k, (colors[k] as string).toLowerCase()])) as ThemeSeeds;
  const derived = deriveTokens(seeds);
  for (const k of TOKENS) if (ok(k)) derived[k] = (colors[k] as string).toLowerCase();
  return derived;
}

/** A custom theme as stored on this device, checked like anything else that could have been edited by hand. */
export function sanitizeCustomTheme(value: unknown): CustomTheme | null {
  if (!value || typeof value !== "object") return null;
  const t = value as Record<string, unknown>;
  const variant = (t.variant ?? {}) as Record<string, unknown>;
  const tokens = readColors(variant.tokens);
  if (typeof t.id !== "string" || !/^custom-[0-9a-z]{4,32}$/.test(t.id) || !tokens) return null;
  return {
    id: t.id,
    name: text(t.name, NAME_MAX) || "Untitled",
    description: text(t.description, DESCRIPTION_MAX) || null,
    builtin: false,
    variant: { tokens, radius: radius(variant.radius) },
    backdrop: t.backdrop ? sanitizeBackdrop(t.backdrop) : null,
    updatedAt: typeof t.updatedAt === "number" ? t.updatedAt : 0,
  };
}

/** A new custom theme starting from any theme's look. */
export function themeFrom(base: Theme, name: string, backdrop: Backdrop | null = null): CustomTheme {
  return {
    id: newThemeId(),
    name: name.slice(0, NAME_MAX),
    description: null,
    builtin: false,
    variant: { tokens: { ...base.variant.tokens }, radius: base.variant.radius },
    backdrop: backdrop ? { ...backdrop } : null,
    updatedAt: Date.now(),
  };
}

// ───────────────────────── Writing ─────────────────────────

/** The file for a theme. `picture` is the backdrop's picture as a data: URL, when it has one to bring along. */
export function toFile(theme: Theme, backdrop: Backdrop | null, picture: string | null): ThemeFile {
  return {
    format: FORMAT,
    version: VERSION,
    name: theme.name,
    description: theme.description ?? "",
    colors: { ...theme.variant.tokens },
    radius: theme.variant.radius,
    backdrop: backdrop ? { ...backdrop, image: picture } : null,
  };
}

export const fileName = (name: string) =>
  `${
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "theme"
  }.fuwa-theme.json`;

/** Reads a picture into a data: URL for a theme file. */
export function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(new Error("couldn't read the picture"));
    reader.readAsDataURL(blob);
  });
}

// ───────────────────────── Reading ─────────────────────────

export class ThemeFileError extends Error {}

/** A data: URL's picture, if it's one instances take and not too big. Nothing else (links included) is read. */
export function pictureFromDataUrl(value: unknown): Blob | null {
  if (typeof value !== "string") return null;
  const match = /^data:(image\/(?:png|jpeg|gif|webp|avif));base64,([A-Za-z0-9+/=\s]+)$/.exec(value);
  if (!match) return null;
  const [, type, data] = match;
  if (!FILE_PICTURE_TYPES.includes(type!) || (data!.length * 3) / 4 > MAX_FILE_PICTURE_BYTES) return null;
  try {
    const bytes = Uint8Array.from(atob(data!.replace(/\s/g, "")), (c) => c.charCodeAt(0));
    return new Blob([bytes], { type });
  } catch {
    return null;
  }
}

/**
 * Reads a theme file. Accepts this format, and plain token sets like
 * waifu.dev's ({ tokens, radius } or { variant: { tokens, radius } }), so a
 * theme copied from the site works too.
 */
export function parseThemeFile(t: I18n["t"], json: string): Imported {
  let data: unknown;
  try {
    data = JSON.parse(json);
  } catch {
    throw new ThemeFileError(t("system.themeFile.notJson"));
  }
  if (!data || typeof data !== "object" || Array.isArray(data)) throw new ThemeFileError(t("system.themeFile.notTheme"));
  const d = data as Record<string, unknown>;
  const notes: string[] = [];
  if (d.format === FORMAT && typeof d.version === "number" && d.version > VERSION) {
    notes.push(t("system.themeFile.newer"));
  }
  const variant = (d.variant && typeof d.variant === "object" ? d.variant : d) as Record<string, unknown>;
  const colors = readColors(d.colors ?? variant.tokens);
  if (!colors) throw new ThemeFileError(t("system.themeFile.noColors"));

  let backdrop: Backdrop | null = null;
  let picture: Blob | null = null;
  if (d.backdrop && typeof d.backdrop === "object") {
    const b = d.backdrop as Record<string, unknown>;
    backdrop = sanitizeBackdrop({ ...b, image: "" });
    if (backdrop.shader && shaderProblem(t, backdrop.shader.code)) {
      notes.push(t("system.themeFile.shaderProblem", { shader: backdrop.shader.name }));
    }
    if (b.image) {
      picture = pictureFromDataUrl(b.image);
      if (!picture) {
        notes.push(
          typeof b.image === "string" && /^https?:/i.test(b.image)
            ? t("system.themeFile.backgroundLink")
            : t("system.themeFile.backgroundUnreadable"),
        );
      }
    }
  }

  const theme: CustomTheme = {
    id: newThemeId(),
    name: text(d.name, NAME_MAX) || t("system.themeFile.importedName"),
    description: text(d.description, DESCRIPTION_MAX) || null,
    builtin: false,
    variant: { tokens: colors, radius: radius(variant.radius ?? d.radius) },
    backdrop,
    updatedAt: Date.now(),
  };
  return { theme, picture, notes };
}
