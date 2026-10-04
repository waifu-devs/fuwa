import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { Effect } from "effect";
import { useEffect, useSyncExternalStore } from "react";
import { GifProvider, MessageGifSchema, type MessageGif } from "@/gen/fuwa/v1/types_pb";
import type { GifCategory, SavedGif, SearchGifsResponse } from "@/gen/fuwa/v1/gif_pb";
import { reportUsage } from "@/lib/reports";
import { call } from "./errors";
import { updateInstance, upsertMessage, withSharedAuthors } from "./store";
import { engine } from "./sync";

/*
 * GIFs: search through the instance (it asks its provider, GIPHY or Klipy,
 * and every picture comes back through it), saved GIFs kept on the
 * instance, and the ones you sent lately, kept on this device.
 */

const api = (key: string) => engine(key).api;

/** Who to credit for results, as their terms ask. */
export function providerName(provider: GifProvider): string {
  return provider === GifProvider.GIPHY ? "GIPHY" : provider === GifProvider.KLIPY ? "Klipy" : "";
}

// ───────────────────────── Whether it's on ─────────────────────────

type Settings = { enabled: boolean; provider: GifProvider; at: number };
const settings = new Map<string, Settings>();
const asking = new Map<string, Promise<void>>();
const listeners = new Set<() => void>();
const changed = () => listeners.forEach((l) => l());
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

function askSettings(key: string) {
  const known = settings.get(key);
  if ((known && Date.now() - known.at < 5 * 60_000) || asking.has(key)) return;
  const ask = api(key)
    .gifs.getGifSettings({})
    .then((r) => void settings.set(key, { enabled: r.enabled, provider: r.provider, at: Date.now() }))
    .catch(() => void settings.set(key, { enabled: false, provider: GifProvider.UNSPECIFIED, at: Date.now() }))
    .finally(() => {
      asking.delete(key);
      changed();
    });
  asking.set(key, ask);
}

/** Whether GIF search is on at an instance, asked once in a while. */
export function useGifSettings(key: string): Settings | undefined {
  useEffect(() => askSettings(key), [key]);
  return useSyncExternalStore(subscribe, () => settings.get(key));
}

// ───────────────────────── Search ─────────────────────────

export const searchGifs = (key: string, query: string, cursor = "") =>
  Effect.gen(function* () {
    reportUsage(query ? "gif.search" : "gif.trending");
    return yield* call<SearchGifsResponse>((signal) => api(key).gifs.searchGifs({ query, cursor, limit: 24 }, { signal }));
  });

const categories = new Map<string, Promise<GifCategory[]>>();

/** Moods to browse, asked once a visit. */
export function gifCategories(key: string): Promise<GifCategory[]> {
  let kept = categories.get(key);
  if (!kept) {
    kept = api(key)
      .gifs.listGifCategories({})
      .then((r) => r.categories);
    kept.catch(() => categories.delete(key));
    categories.set(key, kept);
  }
  return kept;
}

/** A search result stored on the instance, ready to send. */
export const prepareGif = (key: string, resultId: string) =>
  call((signal) => api(key).gifs.prepareGif({ from: { case: "resultId", value: resultId } }, { signal })).pipe(Effect.map((r) => r.gif!));

/** One of your uploaded GIFs, ready to send. */
export const prepareUpload = (key: string, uploadUrl: string) =>
  call((signal) => api(key).gifs.prepareGif({ from: { case: "uploadUrl", value: uploadUrl } }, { signal })).pipe(Effect.map((r) => r.gif!));

// ───────────────────────── Sending ─────────────────────────

/** Sends a GIF on its own; it shows once the server has it. */
export const sendGif = (key: string, serverId: string, channelId: string, gif: MessageGif) =>
  Effect.gen(function* () {
    reportUsage("gif.send");
    rememberSent(key, gif);
    const res = yield* call((signal) => api(key).messages.sendMessage({ serverId, channelId, gif }, { signal }));
    updateInstance(key, (i) => {
      const loaded = i.messages[channelId];
      return {
        ...i,
        users: withSharedAuthors(i.users, [res.message]),
        messages:
          loaded && res.message ? { ...i.messages, [channelId]: { ...loaded, items: upsertMessage(loaded.items, res.message) } } : i.messages,
      };
    });
  });

// ───────────────────────── Saved ─────────────────────────

type Saved = { list: SavedGif[]; loaded: boolean };
const saved = new Map<string, Saved>();
const loading = new Set<string>();

function setSaved(key: string, list: SavedGif[]) {
  saved.set(key, { list, loaded: true });
  changed();
}

/** Your saved GIFs at an instance (favorites and uploads), loaded once. */
export function useSavedGifs(key: string): Saved {
  useEffect(() => {
    if (saved.get(key)?.loaded || loading.has(key)) return;
    loading.add(key);
    api(key)
      .gifs.listSavedGifs({})
      .then((r) => setSaved(key, r.gifs))
      .catch(() => setSaved(key, saved.get(key)?.list ?? []))
      .finally(() => loading.delete(key));
  }, [key]);
  return useSyncExternalStore(subscribe, () => saved.get(key) ?? EMPTY);
}
const EMPTY: Saved = { list: [], loaded: false };

/** Whether a GIF (by its link) is saved. */
export function isSaved(list: SavedGif[], url: string) {
  return list.some((s) => s.gif?.url === url);
}

/** Saves a GIF (one in a message, or your own upload) at the top. */
export const saveGif = (key: string, url: string) =>
  Effect.gen(function* () {
    reportUsage("gif.save");
    const { gif } = yield* call((signal) => api(key).gifs.saveGif({ url }, { signal }));
    if (gif) setSaved(key, [gif, ...(saved.get(key)?.list ?? []).filter((s) => s.gif?.url !== url)]);
    return gif;
  });

/** Takes a GIF off your saved list at once, putting it back if that fails. */
export const unsaveGif = (key: string, url: string) =>
  Effect.gen(function* () {
    const before = saved.get(key)?.list ?? [];
    setSaved(
      key,
      before.filter((s) => s.gif?.url !== url),
    );
    yield* call((signal) => api(key).gifs.deleteSavedGif({ url }, { signal })).pipe(Effect.tapError(() => Effect.sync(() => setSaved(key, before))));
  });

// ───────────────────────── Sent lately (this device) ─────────────────────────

const RECENT = 30;
const recentKey = (key: string) => `fuwa.gifs.recent.${key}`;
const recents = new Map<string, MessageGif[]>();

function readRecent(key: string): MessageGif[] {
  const kept = recents.get(key);
  if (kept) return kept;
  let list: MessageGif[] = [];
  try {
    const raw = JSON.parse(localStorage.getItem(recentKey(key)) ?? "[]") as JsonValue[];
    list = raw.map((g) => fromJson(MessageGifSchema, g, { ignoreUnknownFields: true })).filter((g) => g.url && g.seal);
  } catch {
    list = [];
  }
  recents.set(key, list);
  return list;
}

function rememberSent(key: string, gif: MessageGif) {
  const list = [create(MessageGifSchema, gif), ...readRecent(key).filter((g) => g.url !== gif.url)].slice(0, RECENT);
  recents.set(key, list);
  try {
    localStorage.setItem(recentKey(key), JSON.stringify(list.map((g) => toJson(MessageGifSchema, g))));
  } catch {
    // Not kept past this visit; nothing else depends on it.
  }
  changed();
}

/** The GIFs you sent lately at an instance, from this device. */
export function useRecentGifs(key: string): MessageGif[] {
  return useSyncExternalStore(subscribe, () => readRecent(key));
}
