/**
 * Every emoji someone can use in a server: its own, then their other
 * servers' on the same instance (grouped by server), then the standard set.
 *
 * Another server's emoji goes into a message as its token, and the app sends
 * the emoji along with the message (`SendMessageRequest.emojis`): the
 * instance checks the sender is in that server and stores its picture link
 * with the message, so everyone in the channel sees it. Emoji from other
 * instances aren't offered: their pictures would load from someone else's
 * server.
 *
 * What to offer and how names and searches work is in emoji-search.ts; this
 * file loads the standard set and keeps the catalog, recently used emoji and
 * the skin tone.
 */

import { useMemo, useSyncExternalStore } from "react";
import { useFuwa } from "@/fuwa/store";
import { catalogOf, type Catalog, type StandardGroup } from "@/lib/emoji-search";
import { reportError, reportTiming } from "@/lib/reports";

export * from "@/lib/emoji-search";

// ───────────────────────── The standard set ─────────────────────────

type RawGroup = [id: string, name: string, entries: [char: string, names: string, words: string, skins?: string[]][]];

let standard: StandardGroup[] | null = null;
let loading: Promise<StandardGroup[]> | null = null;
const standardListeners = new Set<() => void>();

/**
 * The standard emoji, bundled with the app (src/lib/emoji-data.json) and
 * loaded the first time something needs them.
 */
export function loadStandard(): Promise<StandardGroup[]> {
  if (standard) return Promise.resolve(standard);
  loading ??= (async () => {
    const started = performance.now();
    try {
      const raw = (await import("@/lib/emoji-data.json")).default as unknown as RawGroup[];
      standard = raw.map(([id, name, entries]) => ({
        id,
        name,
        emojis: entries.map(([char, names, words, skins]) => ({ char, names: names.split(" "), words, skins })),
      }));
      reportTiming("emoji.standard.load", performance.now() - started);
      for (const l of standardListeners) l();
      return standard;
    } catch (err) {
      loading = null;
      reportError(err instanceof Error ? err.name : "Error", "emoji/standard");
      throw err;
    }
  })();
  return loading;
}

/** The standard emoji once they're loaded (starting to load them), else null. */
export function useStandard(): StandardGroup[] | null {
  return useSyncExternalStore(
    (l) => {
      standardListeners.add(l);
      if (!standard) loadStandard().catch(() => {});
      return () => standardListeners.delete(l);
    },
    () => standard,
  );
}

// ───────────────────────── Servers' own ─────────────────────────

/** The catalog for writing in a server, kept until its servers or emoji change. */
export function useCatalog(instanceKey: string, serverId: string): Catalog {
  const servers = useFuwa((s) => s.instances[instanceKey]?.servers);
  const emojis = useFuwa((s) => s.instances[instanceKey]?.emojis);
  return useMemo(() => catalogOf(servers ?? [], emojis ?? {}, serverId), [servers, emojis, serverId]);
}

// ───────────────────────── Recently used and skin tone ─────────────────────────

/**
 * Recently used emoji and the skin tone stay in this browser: a list of keys
 * (`u:` and the emoji, or `c:` and a server emoji's id), newest first.
 */
const RECENT_KEY = "fuwa:emoji:recent";
const TONE_KEY = "fuwa:emoji:tone";
const MAX_RECENT = 24;

function read<T>(key: string, fallback: T, valid: (v: unknown) => v is T): T {
  try {
    const raw = localStorage.getItem(key);
    const value: unknown = raw === null ? fallback : JSON.parse(raw);
    return valid(value) ? value : fallback;
  } catch {
    return fallback;
  }
}

function write(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Private windows and full storage: it's only a convenience.
  }
}

const isKeys = (v: unknown): v is string[] => Array.isArray(v) && v.every((k) => typeof k === "string");
const isTone = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= 5;

let recent = read(RECENT_KEY, [] as string[], isKeys).slice(0, MAX_RECENT);
let tone = read(TONE_KEY, 0, isTone);
const prefListeners = new Set<() => void>();
const subscribe = (l: () => void) => (prefListeners.add(l), () => prefListeners.delete(l));
const notify = () => prefListeners.forEach((l) => l());

export const recentKey = (pick: { char?: string; id?: string }) => (pick.id ? `c:${pick.id}` : `u:${pick.char}`);

/** Moves a picked emoji to the front of the recent ones. */
export function rememberEmoji(key: string) {
  recent = [key, ...recent.filter((k) => k !== key)].slice(0, MAX_RECENT);
  write(RECENT_KEY, recent);
  notify();
}

/** The recently used emoji's keys, newest first. */
export const recentEmoji = () => recent;
export const useSkinTone = () => useSyncExternalStore(subscribe, () => tone);
export function setSkinTone(next: number) {
  tone = isTone(next) ? next : 0;
  write(TONE_KEY, tone);
  notify();
}

