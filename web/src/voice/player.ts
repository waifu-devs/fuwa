import { useSyncExternalStore } from "react";
import { i18n } from "@/i18n/i18n";
import { reportError, reportUsage } from "@/lib/reports";
import { oggToWav, playsOgg } from "./wav";

/**
 * Playing voice messages: one at a time, through one audio element that
 * belongs to the page rather than to a message row, so a message keeps
 * playing while it scrolls out of sight (and its row is dropped) and picks
 * up where it is when it scrolls back. Each opened message is kept as a
 * blob URL, the newest few, so playing it again needs no fetching or
 * opening.
 */

/** Gets a message's sound: the Ogg Opus bytes, already opened on this device. */
export type Loader = () => Promise<Uint8Array<ArrayBuffer>>;

export const RATES = [1, 1.5, 2] as const;

/** "1:05": a voice message's length or place. */
export function clock(ms: number) {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
export type Rate = (typeof RATES)[number];

type State = {
  /** The message in the audio element, playing or paused. */
  current: string | null;
  playing: boolean;
  /** The message being fetched and opened. */
  loading: string | null;
  /** Why a message couldn't be played, by id. */
  failed: Record<string, string>;
  rate: Rate;
};

const RATE_KEY = "fuwa.voice.rate";
const KEPT = 12;

function savedRate(): Rate {
  try {
    const n = Number(localStorage.getItem(RATE_KEY));
    return (RATES as readonly number[]).includes(n) ? (n as Rate) : 1;
  } catch {
    return 1;
  }
}

let state: State = { current: null, playing: false, loading: null, failed: {}, rate: 1 };
let rateLoaded = false;
const listeners = new Set<() => void>();
const set = (patch: Partial<State>) => {
  state = { ...state, ...patch };
  listeners.forEach((l) => l());
};

/** Where the current message is, 0 to 1, for the one row showing it (no re-renders: it sets a transform). */
const positionListeners = new Set<(fraction: number, ms: number) => void>();
/** Where each paused message was left, by id, in ms. */
const leftAt = new Map<string, number>();
/** Opened messages as blob URLs, oldest first. */
const urls = new Map<string, string>();

let audio: HTMLAudioElement | null = null;
let frame = 0;

function element(): HTMLAudioElement {
  if (audio) return audio;
  const a = new Audio();
  a.preload = "auto";
  a.preservesPitch = true;
  a.addEventListener("play", () => {
    set({ playing: true });
    tick();
  });
  a.addEventListener("pause", () => {
    if (state.current) leftAt.set(state.current, a.currentTime * 1000);
    set({ playing: false });
    cancelAnimationFrame(frame);
    emit();
  });
  a.addEventListener("ended", () => {
    if (state.current) leftAt.delete(state.current);
    set({ playing: false });
    cancelAnimationFrame(frame);
    positionListeners.forEach((l) => l(0, 0));
  });
  a.addEventListener("error", () => {
    if (!state.current) return;
    reportError("voice_play", "voice.play");
    set({ playing: false, failed: { ...state.failed, [state.current]: i18n().t("system.voice.cantPlayHereShort") } });
  });
  audio = a;
  return a;
}

function emit() {
  const a = audio;
  if (!a) return;
  const duration = a.duration;
  const fraction = Number.isFinite(duration) && duration > 0 ? a.currentTime / duration : 0;
  positionListeners.forEach((l) => l(fraction, a.currentTime * 1000));
}

function tick() {
  cancelAnimationFrame(frame);
  const step = () => {
    emit();
    if (state.playing) frame = requestAnimationFrame(step);
  };
  frame = requestAnimationFrame(step);
}

async function urlFor(id: string, load: Loader): Promise<string> {
  const kept = urls.get(id);
  if (kept) {
    // Most recent last.
    urls.delete(id);
    urls.set(id, kept);
    return kept;
  }
  const started = performance.now();
  const ogg = await load();
  const bytes = playsOgg() ? ogg : await oggToWav(ogg);
  const url = URL.createObjectURL(new Blob([bytes], { type: playsOgg() ? "audio/ogg" : "audio/wav" }));
  urls.set(id, url);
  while (urls.size > KEPT) {
    const [oldest, old] = urls.entries().next().value!;
    if (oldest === state.current) break;
    urls.delete(oldest);
    URL.revokeObjectURL(old);
  }
  void started;
  return url;
}

/** Plays a message (from where it was left), or pauses it if it's the one playing. */
export async function toggle(id: string, load: Loader, at?: number) {
  const a = element();
  if (!rateLoaded) {
    rateLoaded = true;
    set({ rate: savedRate() });
  }
  if (state.current === id && at === undefined) {
    if (a.paused) await a.play().catch(() => {});
    else a.pause();
    return;
  }
  if (state.loading === id) return;
  set({ loading: id });
  try {
    const url = await urlFor(id, load);
    if (state.loading !== id) return;
    if (state.current !== id) {
      if (!a.paused) a.pause();
      a.src = url;
      set({ current: id });
    }
    a.playbackRate = state.rate;
    a.defaultPlaybackRate = state.rate;
    const from = at ?? leftAt.get(id) ?? 0;
    if (Math.abs(a.currentTime * 1000 - from) > 30) a.currentTime = from / 1000;
    set({ loading: null });
    reportUsage("voice.play");
    await a.play();
  } catch (err) {
    if (state.loading === id) set({ loading: null });
    if (err instanceof DOMException && err.name === "AbortError") return;
    reportError("voice_open", "voice.play");
    const failed = err instanceof Error && err.message ? err.message : i18n().t("system.voice.cantPlay");
    set({ failed: { ...state.failed, [id]: failed.charAt(0).toUpperCase() + failed.slice(1).replace(/\.?$/, ".") } });
  }
}

/** Moves a message to a point (0 to 1) of `durationMs`, playing it if it's not the current one. */
export function seek(id: string, fraction: number, durationMs: number, load: Loader) {
  const ms = Math.max(0, Math.min(1, fraction)) * durationMs;
  if (state.current === id && audio) {
    audio.currentTime = ms / 1000;
    leftAt.set(id, ms);
    emit();
    return;
  }
  void toggle(id, load, ms);
}

/** Steps through 1×, 1.5× and 2×, for every message from now on. */
export function nextRate() {
  const rate = RATES[(RATES.indexOf(state.rate) + 1) % RATES.length]!;
  try {
    localStorage.setItem(RATE_KEY, String(rate));
  } catch {
    // Kept for this page only.
  }
  if (audio) {
    audio.playbackRate = rate;
    audio.defaultPlaybackRate = rate;
  }
  set({ rate });
}

/** Stops whatever's playing (a recording is starting, say). */
export function pauseAll() {
  audio?.pause();
}

/** Where a message was left, in ms, if it was started and paused. */
export const positionOf = (id: string) => (state.current === id && audio ? audio.currentTime * 1000 : (leftAt.get(id) ?? 0));

/** Hears where the current message is, until the returned function is called. */
export function onPosition(listener: (fraction: number, ms: number) => void): () => void {
  positionListeners.add(listener);
  return () => positionListeners.delete(listener);
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

/** One message's playback, for its row. Rows for other messages don't re-render as it plays. */
export function useVoice(id: string) {
  const current = useSyncExternalStore(subscribe, () => state.current === id);
  const playing = useSyncExternalStore(subscribe, () => state.current === id && state.playing);
  const loading = useSyncExternalStore(subscribe, () => state.loading === id);
  const failed = useSyncExternalStore(subscribe, () => state.failed[id] ?? "");
  const rate = useSyncExternalStore(subscribe, () => state.rate);
  return { current, playing, loading, failed, rate };
}
