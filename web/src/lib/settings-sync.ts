import type { Prefs } from "@/lib/prefs";

/**
 * App settings on every device (docs/app-settings.md), the pure part: which
 * settings travel, the document instances keep, and merging it with this
 * device's. `fuwa/app-settings.ts` does the talking.
 */

export const FORMAT = "fuwa.app-settings.v1";

/** The settings that travel; the rest belong to this device's screen and hardware. */
export const SYNCED = [
  "theme",
  "followSystem",
  "lightTheme",
  "darkTheme",
  "customThemes",
  "backdrop",
  "density",
  "messageDisplay",
  "chatFontSize",
  "font",
  "codeFont",
  "reduceMotion",
  "othersEffects",
  "saturation",
  "colorVision",
  "colorVisionStrength",
  "highContrast",
  "underlineLinks",
  "roleColors",
  "language",
  "clock",
  "sendWith",
  "notifyFor",
  "unreadBadge",
  "sounds",
  "volume",
  "streamerHidePersonal",
  "streamerMuteSounds",
  "streamerMuteNotifications",
  "keybinds",
  "customKeybinds",
  "developerMode",
  "shareReports",
  "sendTyping",
  "gachaSounds",
  "inputMode",
  "pttRelease",
  "userVolumes",
  "mirrorVideo",
  "cameraHeight",
  "cameraFps",
  "popoutName",
  "popoutGlow",
  "popoutFit",
  "shareSound",
  "shareSurface",
  "shareHeight",
  "shareFps",
  "trustedDomains",
] as const satisfies readonly (keyof Prefs)[];

export type SyncedName = (typeof SYNCED)[number];

const isSynced = (name: string): name is SyncedName => (SYNCED as readonly string[]).includes(name);

/** One setting as kept: its value and when it last changed, in unix milliseconds. */
export type Entry = { at: number; value: unknown };
export type Entries = Record<string, Entry>;
/** When each setting last changed here, or the stamp of the value taken from elsewhere. */
export type Stamps = Partial<Record<SyncedName, number>>;

/** Whether a value may leave the device: a font imported from a file lives only here. */
export function shareable(name: string, value: unknown): boolean {
  if (name === "font" || name === "codeFont") return (value as { kind?: unknown } | null)?.kind !== "file";
  return value !== undefined;
}

/** Reads an instance's document; anything malformed reads as nothing. */
export function readDocument(text: string): Entries {
  if (!text) return {};
  try {
    const doc = JSON.parse(text) as { format?: unknown; settings?: unknown };
    if (doc.format !== FORMAT || !doc.settings || typeof doc.settings !== "object") return {};
    const entries: Entries = {};
    for (const [name, entry] of Object.entries(doc.settings as Record<string, unknown>)) {
      const e = entry as Partial<Entry> | null;
      if (e && typeof e === "object" && typeof e.at === "number" && Number.isFinite(e.at) && "value" in e) entries[name] = { at: e.at, value: e.value };
    }
    return entries;
  } catch {
    return {};
  }
}

export function writeDocument(entries: Entries): string {
  return JSON.stringify({ format: FORMAT, settings: entries });
}

/** The same value, whatever order its keys are in. */
export function sameValue(a: unknown, b: unknown): boolean {
  return canonical(a) === canonical(b);
}

function canonical(value: unknown): string {
  return JSON.stringify(value, (_, v: unknown) =>
    v && typeof v === "object" && !Array.isArray(v)
      ? Object.fromEntries(Object.entries(v as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
      : v,
  );
}

/** This device's settings as entries: only those changed here or taken from elsewhere. */
export function localEntries(prefs: Prefs, stamps: Stamps): Entries {
  const entries: Entries = {};
  for (const name of SYNCED) {
    const at = stamps[name];
    if (at && shareable(name, prefs[name])) entries[name] = { at, value: prefs[name] };
  }
  return entries;
}

/**
 * Stamps the synced settings that changed: now, or just past the stamp they
 * had, so a change made after seeing another device's never loses to it.
 */
export function stamp(before: Prefs, after: Prefs, stamps: Stamps, now: number): Stamps {
  let next = stamps;
  for (const name of SYNCED) {
    if (sameValue(before[name], after[name])) continue;
    if (next === stamps) next = { ...stamps };
    next[name] = Math.max(now, (stamps[name] ?? 0) + 1);
  }
  return next;
}

/**
 * The first stamps on a device that had settings before they travelled:
 * those it changed from the defaults, as old as can be, so an instance
 * that already has them wins and one that doesn't gets them.
 */
export function firstStamps(prefs: Prefs, defaults: Prefs): Stamps {
  const stamps: Stamps = {};
  for (const name of SYNCED) if (!sameValue(prefs[name], defaults[name])) stamps[name] = 1;
  return stamps;
}

/**
 * Merges this device's entries with an instance's, setting by setting, the
 * later stamp winning (the instance's on a tie). `incoming` is what this
 * device should take (with the stamps to keep, even when the value is the same); `behind` says the instance lacks something newer.
 * Settings this app doesn't know stay as the instance has them.
 */
export function merge(local: Entries, remote: Entries): { merged: Entries; incoming: Entries; behind: boolean } {
  const merged: Entries = { ...remote };
  const incoming: Entries = {};
  let behind = false;
  for (const [name, mine] of Object.entries(local)) {
    const theirs = remote[name];
    if (!theirs || mine.at > theirs.at) {
      merged[name] = mine;
      behind = true;
    }
  }
  for (const [name, theirs] of Object.entries(remote)) {
    if (!isSynced(name) || !shareable(name, theirs.value)) continue;
    const mine = local[name];
    if (!mine || theirs.at > mine.at) incoming[name] = theirs;
  }
  return { merged, incoming, behind };
}
