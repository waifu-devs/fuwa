import { useSyncExternalStore } from "react";

/**
 * Each person's own say over live tiles (lib/live-tiles.ts), kept in this
 * browser: whether they show at all (on unless turned off), servers they're
 * off in, and tiles someone hid. Which kinds a server shows is the server's
 * setting, not this.
 */
const OFF_KEY = "fuwa:live-tiles:off";
const HIDDEN_KEY = "fuwa:live-tiles:hidden";
const QUIET_KEY = "fuwa:live-tiles:quiet-servers";
const MAX_HIDDEN = 200;

function readOff(): boolean {
  try {
    return localStorage.getItem(OFF_KEY) === "1";
  } catch {
    return false;
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

function write(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Storage full or blocked: the choice lasts until the page reloads.
  }
}

type Local = { on: boolean; hidden: ReadonlySet<string>; quiet: ReadonlySet<string> };
let local: Local | null = null;
const listeners = new Set<() => void>();
const get = (): Local => (local ??= { on: !readOff(), hidden: new Set(readList(HIDDEN_KEY)), quiet: new Set(readList(QUIET_KEY)) });
const set = (next: Local) => {
  local = next;
  for (const l of listeners) l();
};
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

export const useLiveTilesLocal = (): Local => useSyncExternalStore(subscribe, get);

/** Turns tiles on or off everywhere, on this device. */
export function setTilesOn(on: boolean) {
  write(OFF_KEY, on ? null : "1");
  set({ ...get(), on });
}

/** Hides one tile on this device; the newest hidden ones are kept. */
export function hideTile(id: string) {
  const hidden = [...get().hidden.values(), id].slice(-MAX_HIDDEN);
  write(HIDDEN_KEY, JSON.stringify(hidden));
  set({ ...get(), hidden: new Set(hidden) });
}

/** Turns tiles off (or back on) for one server, on this device. */
export function setServerQuiet(serverId: string, quiet: boolean) {
  const next = new Set(get().quiet);
  if (quiet) next.add(serverId);
  else next.delete(serverId);
  write(QUIET_KEY, JSON.stringify([...next]));
  set({ ...get(), quiet: next });
}
