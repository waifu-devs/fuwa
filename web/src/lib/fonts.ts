/**
 * The fonts someone picks for the app and for code: an app setting, so it's
 * this device's alone and never reaches an instance. A font is the bundled
 * one, the system's, one installed on this computer (by its family name), or
 * a file imported here (kept in this browser, `font-files.ts`).
 *
 * Pure, so it can be tested; the page side is `font-files.ts` and `prefs.ts`.
 */

export type FontPick =
  | { kind: "default" }
  | { kind: "system" }
  | { kind: "installed"; family: string }
  | { kind: "file"; id: string; name: string };

/** Which setting a pick is for: the app's text, or fixed-width text. */
export type FontRole = "app" | "code";

export const DEFAULT_FONT: FontPick = { kind: "default" };

/** What each role falls back to: the bundled font, or the system's monospace. */
export const BASE_STACK: Record<FontRole, string> = {
  app: '"M PLUS Rounded 1c", ui-rounded, ui-sans-serif, system-ui, sans-serif',
  code: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
};

const SYSTEM_STACK = "system-ui, ui-sans-serif, sans-serif";

/** The biggest font file kept: CJK fonts run to tens of megabytes. */
export const MOST_FONT_BYTES = 32 * 1024 * 1024;
/** What a font file may be, as the file picker and the error say. */
export const FONT_EXTENSIONS = [".ttf", ".otf", ".ttc", ".woff", ".woff2"] as const;
export const FONT_TYPES = "TTF, OTF, WOFF, WOFF2";

const MOST_FAMILY = 100;
const MOST_NAME = 80;

/** A family name as typed or listed: no control characters, trimmed, not too long. */
export function cleanFamily(value: unknown): string {
  if (typeof value !== "string") return "";
  return value.replace(/[\u0000-\u001f\u007f]/g, "").trim().slice(0, MOST_FAMILY);
}

/** A stored pick from an older or hand-edited version, made into one this app can use. */
export function sanitizeFont(value: unknown, role: FontRole): FontPick {
  if (!value || typeof value !== "object") return DEFAULT_FONT;
  const v = value as Record<string, unknown>;
  switch (v.kind) {
    case "system":
      // Code has no system font of its own: its default is the system's monospace.
      return role === "app" ? { kind: "system" } : DEFAULT_FONT;
    case "installed": {
      const family = cleanFamily(v.family);
      return family ? { kind: "installed", family } : DEFAULT_FONT;
    }
    case "file": {
      const id = typeof v.id === "string" && /^[a-z0-9]{1,32}$/.test(v.id) ? v.id : "";
      const name = typeof v.name === "string" ? v.name.slice(0, MOST_NAME) : "";
      return id ? { kind: "file", id, name } : DEFAULT_FONT;
    }
    default:
      return DEFAULT_FONT;
  }
}

/** A family name as a CSS string, whatever characters it has. */
export const quoteFamily = (family: string) => `"${family.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;

/** The family an imported file is registered under in the page. */
export const fileFamily = (id: string) => `fuwa-font-${id}`;

/**
 * The CSS font-family for a pick. A picked font comes first and the role's
 * own after it, so letters it lacks (Japanese, often) still show.
 */
export function fontStack(pick: FontPick, role: FontRole): string {
  const base = BASE_STACK[role];
  switch (pick.kind) {
    case "default":
      return base;
    case "system":
      return role === "app" ? SYSTEM_STACK : base;
    case "installed":
      return `${quoteFamily(pick.family)}, ${base}`;
    case "file":
      return `${quoteFamily(fileFamily(pick.id))}, ${base}`;
  }
}

/** Whether a file looks like a font by its name, before reading it. */
export const fontFileName = (name: string) => FONT_EXTENSIONS.some((ext) => name.toLowerCase().endsWith(ext));

/** Families listed by the browser (one entry per face), each once and in order. */
export function familiesOf(faces: { family: string }[]): string[] {
  const seen = new Set<string>();
  for (const face of faces) {
    const family = cleanFamily(face.family);
    if (family) seen.add(family);
  }
  return [...seen].sort((a, b) => a.localeCompare(b));
}

/** Families matching a search, those starting with it first. */
export function searchFamilies(families: string[], query: string, most = 200): string[] {
  const q = query.trim().toLowerCase();
  if (!q) return families.slice(0, most);
  const starts: string[] = [];
  const has: string[] = [];
  for (const f of families) {
    const at = f.toLowerCase().indexOf(q);
    if (at === 0) starts.push(f);
    else if (at > 0) has.push(f);
  }
  return [...starts, ...has].slice(0, most);
}
