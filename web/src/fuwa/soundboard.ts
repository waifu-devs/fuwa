import { useEffect, useSyncExternalStore } from "react";
import type { Event, Sound } from "@/gen/fuwa/v1/types_pb";
import { getCalls } from "@/calls/state";
import { audioContext } from "@/calls/audio";
import { i18n } from "@/i18n/i18n";
import { getPrefs } from "@/lib/prefs";
import { reportUsage } from "@/lib/reports";
import { shownPicture } from "@/lib/shown";
import { heardHere, MAX_SOUND_BYTES, playVolume, sortSounds } from "@/lib/soundboard";
import { readExactly } from "@/voice/fetch";
import { oggToWav } from "@/voice/wav";
import { toFuwaError } from "./errors";
import { engine } from "./sync";

/*
 * The soundboard (docs/soundboard.md): each server's sounds, listed when
 * someone opens the soundboard or its settings page and kept in step with
 * SoundsUpdated; and SoundPlayed, which plays the sound here when this app
 * is in that voice channel's call (and not deafened), and shows for a
 * moment who played what. Kept apart from the main store, like pins.
 */

export type ServerSounds = { status: "loading" | "ready" | "failed"; sounds: Sound[] };

const lists = new Map<string, ServerSounds>();
const listeners = new Set<() => void>();
const notify = () => {
  for (const l of listeners) l();
};
const subscribe = (l: () => void) => (listeners.add(l), () => listeners.delete(l));
const keyOf = (instanceKey: string, serverId: string) => `${instanceKey}|${serverId}`;

function put(key: string, value: ServerSounds) {
  lists.set(key, value);
  notify();
}

/** A server's sounds, once listed. Lists them the first time it's used for that server. */
export function useSounds(instanceKey: string, serverId: string): ServerSounds | undefined {
  const list = useSyncExternalStore(subscribe, () => lists.get(keyOf(instanceKey, serverId)));
  useEffect(() => {
    if (serverId && !lists.has(keyOf(instanceKey, serverId))) void loadSounds(instanceKey, serverId);
  }, [instanceKey, serverId]);
  return list;
}

/** Lists a server's sounds (again). */
export async function loadSounds(instanceKey: string, serverId: string) {
  const key = keyOf(instanceKey, serverId);
  const had = lists.get(key);
  put(key, { status: "loading", sounds: had?.sounds ?? [] });
  try {
    const res = await engine(instanceKey).api.soundboard.listSounds({ serverId });
    // An event may have brought a newer list meanwhile; it's the whole list either way.
    if (lists.get(key)?.status === "loading") put(key, { status: "ready", sounds: sortSounds(res.sounds) });
  } catch {
    put(key, { status: "failed", sounds: had?.sounds ?? [] });
  }
}

/** Puts a sound the instance answered with into its server's list, as the event will. */
function upsert(instanceKey: string, sound: Sound | undefined) {
  if (!sound) return;
  const key = keyOf(instanceKey, sound.serverId);
  const list = lists.get(key);
  if (!list) return;
  put(key, { ...list, sounds: sortSounds([...list.sounds.filter((s) => s.id !== sound.id), sound]) });
}

export async function createSound(
  instanceKey: string,
  req: { serverId: string; name: string; url: string; emoji: string; volume: number; durationMs: number },
): Promise<Sound | undefined> {
  reportUsage("soundboard.create");
  try {
    const { sound } = await engine(instanceKey).api.soundboard.createSound(req);
    upsert(instanceKey, sound);
    return sound;
  } catch (err) {
    throw toFuwaError(err);
  }
}

export async function updateSound(instanceKey: string, serverId: string, soundId: string, change: { name?: string; emoji?: string; volume?: number }) {
  try {
    const { sound } = await engine(instanceKey).api.soundboard.updateSound({ serverId, soundId, ...change });
    upsert(instanceKey, sound);
  } catch (err) {
    throw toFuwaError(err);
  }
}

export async function deleteSound(instanceKey: string, serverId: string, soundId: string) {
  try {
    await engine(instanceKey).api.soundboard.deleteSound({ serverId, soundId });
    const key = keyOf(instanceKey, serverId);
    const list = lists.get(key);
    if (list) put(key, { ...list, sounds: list.sounds.filter((s) => s.id !== soundId) });
  } catch (err) {
    throw toFuwaError(err);
  }
}

/** Plays a sound for everyone in the voice channel you're in; you hear it when the instance says it played, like everyone. */
export async function playSound(instanceKey: string, serverId: string, channelId: string, soundId: string) {
  reportUsage("soundboard.play");
  try {
    await engine(instanceKey).api.soundboard.playSound({ serverId, channelId, soundId });
  } catch (err) {
    throw toFuwaError(err);
  }
}

// ─────────────── Files, decoded once a session ───────────────

const decoded = new Map<string, Promise<AudioBuffer>>();

/** A sound's file, fetched from its instance (never anywhere else, never bigger than it says) and decoded. */
export function soundBuffer(sound: Pick<Sound, "url" | "size">): Promise<AudioBuffer> {
  let buffer = decoded.get(sound.url);
  if (!buffer) {
    buffer = fetchAndDecode(sound.url, Number(sound.size));
    decoded.set(sound.url, buffer);
    // A failure is forgotten, so playing it again tries again.
    buffer.catch(() => decoded.delete(sound.url));
  }
  return buffer;
}

async function fetchAndDecode(url: string, size: number): Promise<AudioBuffer> {
  const cantPlay = () => new Error(i18n().t("dms-calls.calls.soundboard.cantPlay"));
  const from = shownPicture(url);
  if (!from || !(size > 0) || size > MAX_SOUND_BYTES) throw cantPlay();
  const res = await fetch(from, { credentials: "omit", referrerPolicy: "no-referrer" });
  if (!res.ok || !res.body) throw cantPlay();
  const bytes = await readExactly(res.body, size).catch(() => {
    throw cantPlay();
  });
  const ctx = audioContext();
  try {
    // decodeAudioData takes (and detaches) its buffer, so it gets a copy.
    return await ctx.decodeAudioData(bytes.slice().buffer);
  } catch {
    // Browsers whose WebAudio can't read Ogg Opus: decoded by WebCodecs into WAV first.
    try {
      const wav = await oggToWav(bytes);
      return await ctx.decodeAudioData(wav.slice().buffer);
    } catch {
      throw cantPlay();
    }
  }
}

/** Plays a sound for you alone (the soundboard's preview), at its volume and yours. Answers a way to stop it. */
export async function previewSound(sound: Sound, onEnd?: () => void): Promise<() => void> {
  const buffer = await soundBuffer(sound);
  const { playThroughCall } = await import("@/calls/engine");
  return playThroughCall(buffer, playVolume(sound.volume, getPrefs().soundboardVolume), onEnd);
}

// ─────────────── Who played what ───────────────

/** A sound someone just played, shown by their tile for a moment. */
export type Played = { n: number; sound: Sound; channelId: string };

const SHOWN_MS = 3_200;
const played = new Map<string, Played>();
let playedCount = 0;
const playedKey = (instanceKey: string, serverId: string, userId: string) => `${instanceKey}|${serverId}|${userId}`;

/** The sound someone played last in a voice channel, while it's shown. */
export function useJustPlayed(instanceKey: string, serverId: string, channelId: string, userId: string): Played | undefined {
  const p = useSyncExternalStore(subscribe, () => played.get(playedKey(instanceKey, serverId, userId)));
  return p?.channelId === channelId ? p : undefined;
}

function showPlayed(instanceKey: string, serverId: string, userId: string, entry: Played) {
  const key = playedKey(instanceKey, serverId, userId);
  played.set(key, entry);
  notify();
  setTimeout(() => {
    if (played.get(key)?.n !== entry.n) return;
    played.delete(key);
    notify();
  }, SHOWN_MS);
}

/** A server's event: its sounds changed, or someone played one. */
export function onSoundboardEvent(instanceKey: string, event: Event) {
  const p = event.payload;
  if (p.case === "soundsUpdated") {
    put(keyOf(instanceKey, event.serverId), { status: "ready", sounds: sortSounds(p.value.sounds) });
  } else if (p.case === "soundPlayed") {
    const { channelId, userId, sound } = p.value;
    if (!sound) return;
    showPlayed(instanceKey, event.serverId, userId, { n: ++playedCount, sound, channelId });
    const calls = getCalls();
    if (!heardHere(calls.call?.target, instanceKey, event.serverId, channelId, calls.selfDeaf)) return;
    void (async () => {
      try {
        const buffer = await soundBuffer(sound);
        // Still there, and still hearing, once it's ready.
        const now = getCalls();
        if (!heardHere(now.call?.target, instanceKey, event.serverId, channelId, now.selfDeaf)) return;
        const { playThroughCall } = await import("@/calls/engine");
        playThroughCall(buffer, playVolume(sound.volume, getPrefs().soundboardVolume));
      } catch {
        // A sound that can't be fetched or read stays quiet; the bubble still says it was played.
      }
    })();
  }
}

/** The stream (re)started for a server: a list someone opened is read again, since changes meanwhile never came. */
export function refreshSounds(instanceKey: string, serverId: string) {
  if (lists.has(keyOf(instanceKey, serverId))) void loadSounds(instanceKey, serverId);
}

/** A server you left: its sounds go. */
export function forgetSounds(instanceKey: string, serverId: string) {
  if (lists.delete(keyOf(instanceKey, serverId))) notify();
}
