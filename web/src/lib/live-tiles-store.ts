import { useSyncExternalStore } from "react";
import type { KindChoices, TileKind } from "./live-tiles";

/**
 * Whether live tiles (lib/live-tiles.ts) show in this browser, and what
 * someone hid. Nothing here reaches a server.
 *
 * off: nothing changes (the default). on: tiles from real activity. demo:
 * real ones plus samples. `?live-tiles=on|demo|off` in the address sets it.
 */
export type TilesMode = "off" | "on" | "demo";

const MODE_KEY = "fuwa:live-tiles";
const HIDDEN_KEY = "fuwa:live-tiles:hidden";
const QUIET_KEY = "fuwa:live-tiles:quiet-servers";
const KINDS_KEY = "fuwa:live-tiles:server-kinds";
const MAX_HIDDEN = 200;

const isMode = (v: unknown): v is TilesMode => v === "off" || v === "on" || v === "demo";

function readMode(): TilesMode {
  try {
    const asked = new URLSearchParams(location.search).get("live-tiles");
    if (isMode(asked)) localStorage.setItem(MODE_KEY, asked);
    const saved = localStorage.getItem(MODE_KEY);
    return isMode(saved) ? saved : "off";
  } catch {
    return "off";
  }
}

function readList(key: string): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(key) ?? "[]") as unknown;
    return Array.isArray(raw) ? raw.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function writeList(key: string, list: string[]) {
  try {
    localStorage.setItem(key, JSON.stringify(list));
  } catch {
    // Storage full or blocked: the choice lasts until the page reloads.
  }
}

// Read as the app starts, before signing in or routing drops the address's query.
const startMode = readMode();

type Local = { mode: TilesMode; hidden: ReadonlySet<string>; quiet: ReadonlySet<string>; kinds: Readonly<Record<string, KindChoices>> };
let local: Local | null = null;
const listeners = new Set<() => void>();
const get = (): Local => (local ??= { mode: startMode, hidden: new Set(readList(HIDDEN_KEY)), quiet: new Set(readList(QUIET_KEY)), kinds: readKinds() });
const set = (next: Local) => {
  local = next;
  for (const l of listeners) l();
};
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

export const useLiveTilesLocal = (): Local => useSyncExternalStore(subscribe, get);

export function setTilesMode(mode: TilesMode) {
  try {
    localStorage.setItem(MODE_KEY, mode);
  } catch {
    // As above.
  }
  set({ ...get(), mode });
}

/** Hides one tile on this device; the newest hidden ones are kept. */
export function hideTile(id: string) {
  const hidden = [...get().hidden.values(), id].slice(-MAX_HIDDEN);
  writeList(HIDDEN_KEY, hidden);
  set({ ...get(), hidden: new Set(hidden) });
}

/** Turns tiles off (or back on) for one server, on this device. */
export function setServerQuiet(serverId: string, quiet: boolean) {
  const next = new Set(get().quiet);
  if (quiet) next.add(serverId);
  else next.delete(serverId);
  writeList(QUIET_KEY, [...next]);
  set({ ...get(), quiet: next });
}

function readKinds(): Record<string, KindChoices> {
  try {
    const raw = JSON.parse(localStorage.getItem(KINDS_KEY) ?? "{}") as unknown;
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
    const out: Record<string, KindChoices> = {};
    for (const [server, choices] of Object.entries(raw)) {
      if (!choices || typeof choices !== "object") continue;
      out[server] = Object.fromEntries(Object.entries(choices).filter(([, on]) => typeof on === "boolean")) as KindChoices;
    }
    return out;
  } catch {
    return {};
  }
}

/**
 * Turns one kind of tile on or off for a whole server. In this draft it's kept
 * in this browser, standing in for a server setting its admins (Manage Server)
 * would change for everyone.
 */
export function setServerKind(serverId: string, kind: TileKind, on: boolean) {
  const kinds = { ...get().kinds, [serverId]: { ...get().kinds[serverId], [kind]: on } };
  try {
    localStorage.setItem(KINDS_KEY, JSON.stringify(kinds));
  } catch {
    // As above.
  }
  set({ ...get(), kinds });
}
