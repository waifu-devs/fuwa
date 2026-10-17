/**
 * The soundboard's pure parts (docs/soundboard.md): whether a sound someone
 * played is heard here and how loud, the part of a file a new sound keeps,
 * and the sums that make its Ogg file say exactly how long it plays.
 */

/** The longest a sound plays, as the instance checks it (`MAX_SOUND_MS`). */
export const MAX_SOUND_MS = 10_000;
/** The biggest a sound's file can be; the instance refuses more before it's sent. */
export const MAX_SOUND_BYTES = 1024 * 1024;
/** Sounds are made and played at 48 kHz, Opus's own rate. */
export const SOUND_RATE = 48_000;
/** A sound's name, as the instance takes it. */
export const MAX_SOUND_NAME = 32;

/** The call this app is in, as far as hearing a sound goes. */
export type HearingCall = { kind: "voice"; instance: string; serverId: string; channelId: string } | { kind: "dm" } | null | undefined;

/**
 * Whether a sound played in `channelId` (of `serverId`, on `instance`) is
 * heard here: only in that voice channel's call, and not while deafened.
 */
export function heardHere(call: HearingCall, instance: string, serverId: string, channelId: string, deafened: boolean): boolean {
  return !deafened && !!call && call.kind === "voice" && call.instance === instance && call.serverId === serverId && call.channelId === channelId;
}

/** How loud a sound plays: its own volume times the person's soundboard volume, each 0 to 1. */
export function playVolume(soundVolume: number, mine: number): number {
  const unit = (n: number, fallback: number) => (Number.isFinite(n) ? Math.min(1, Math.max(0, n)) : fallback);
  return unit(soundVolume, 1) * unit(mine, 1);
}

/** Why you can't play a sound right now, or null when you can. */
export type Blocked = "connecting" | "noSpeak" | "serverMuted" | "deafened" | "muted" | null;

export function playBlocked(s: { connected: boolean; speak: boolean; selfMute: boolean; selfDeaf: boolean; serverMute: boolean; serverDeaf: boolean; suppress: boolean }): Blocked {
  if (!s.speak || s.suppress) return "noSpeak";
  if (s.serverMute || s.serverDeaf) return "serverMuted";
  if (s.selfDeaf) return "deafened";
  if (s.selfMute) return "muted";
  if (!s.connected) return "connecting";
  return null;
}

/**
 * The part of a file a new sound keeps, in milliseconds: from `start`
 * (moved so the window fits) for at most `max`, never past the end.
 */
export function clampWindow(totalMs: number, startMs: number, max = MAX_SOUND_MS): { startMs: number; endMs: number } {
  const total = Math.max(0, Number.isFinite(totalMs) ? totalMs : 0);
  const length = Math.min(total, max);
  const start = Math.min(Math.max(0, Number.isFinite(startMs) ? startMs : 0), total - length);
  return { startMs: start, endMs: start + length };
}

/** Where a window falls in frames at `rate`: the first frame and how many, never more than `max` milliseconds' worth. */
export function windowFrames(startMs: number, endMs: number, rate = SOUND_RATE, max = MAX_SOUND_MS): { from: number; frames: number } {
  const from = Math.max(0, Math.round((startMs / 1000) * rate));
  const frames = Math.max(0, Math.min(Math.round(((endMs - startMs) / 1000) * rate), Math.floor((max / 1000) * rate)));
  return { from, frames };
}

/** How long `frames` at 48 kHz play, in whole milliseconds, as the instance reads it from the file. */
export const framesToMs = (frames: number) => Math.floor((Math.max(0, frames) * 1000) / SOUND_RATE);

/**
 * How many samples to put against each Opus packet, so that the file's last
 * granule position, less the pre-skip, is exactly `frames`: the encoder
 * pads its last packet, and the decoder drops the first `preSkip` samples,
 * so the packets cover more than the sound. Packets wholly past what the
 * sound needs go, and the extra comes off the end (RFC 7845 lets the last
 * page end early).
 */
export function trimPackets(durations: readonly number[], frames: number, preSkip: number): number[] {
  const needed = frames + preSkip;
  const kept: number[] = [];
  let covered = 0;
  for (const d of durations) {
    if (covered >= needed) break;
    kept.push(d);
    covered += d;
  }
  // `writeOggOpus` counts granules from the pre-skip, so the samples given add up to `frames`.
  let extra = kept.reduce((n, d) => n + d, 0) - frames;
  for (let i = kept.length - 1; i >= 0 && extra > 0; i--) {
    const off = Math.min(kept[i]!, extra);
    kept[i] = kept[i]! - off;
    extra -= off;
  }
  return kept;
}

/** A short fade in and out, in frames, so a cut doesn't click. */
export const FADE_FRAMES = 240;

/** A sound's name from its file's: no extension, no underscores, at most 32 characters; "" for none. */
export function soundNameFromFile(file: string): string {
  const base = file.replace(/\.[^./\\]{1,8}$/, "");
  const name = base.replace(/[_]+/g, " ").replace(/\s+/g, " ").trim();
  return [...name].slice(0, MAX_SOUND_NAME).join("").trim();
}

/** The name, with " 2", " 3"... added until no sound has it, whatever the case. */
export function freeName(name: string, taken: readonly string[]): string {
  const used = new Set(taken.map((n) => n.toLowerCase()));
  if (!used.has(name.toLowerCase())) return name;
  for (let n = 2; ; n++) {
    const tail = ` ${n}`;
    const next = [...name].slice(0, MAX_SOUND_NAME - tail.length).join("").trimEnd() + tail;
    if (!used.has(next.toLowerCase())) return next;
  }
}

/** Whether the instance takes this as a sound's name. */
export const validName = (name: string) => {
  const n = [...name.trim()].length;
  return n >= 1 && n <= MAX_SOUND_NAME;
};

const TOKEN = /^<a?:([A-Za-z0-9_]{2,32}):([0-9A-Za-z]{10,32})>$/;

/**
 * A picked emoji as a sound keeps it: a Unicode emoji as it is, a server's
 * own as `<:name:id>` (the instance doesn't take the `<a:` of moving ones;
 * the emoji itself says whether it moves).
 */
export function soundEmoji(text: string): string {
  const t = text.trim();
  const token = TOKEN.exec(t);
  return token ? `<:${token[1]}:${token[2]}>` : t;
}

/** A sound's emoji, read: a server emoji's id and name, or the Unicode text, or nothing. */
export function readEmoji(text: string): { kind: "custom"; id: string; name: string } | { kind: "unicode"; text: string } | null {
  const t = text.trim();
  if (!t) return null;
  const token = TOKEN.exec(t);
  return token ? { kind: "custom", name: token[1]!, id: token[2]! } : { kind: "unicode", text: t };
}

/** Sounds in the order a server lists them: oldest first, then by id. */
export function sortSounds<T extends { id: string; createdAt?: { seconds: bigint; nanos: number } }>(sounds: readonly T[]): T[] {
  const at = (s: T) => (s.createdAt ? Number(s.createdAt.seconds) * 1000 + Math.floor(s.createdAt.nanos / 1e6) : 0);
  return [...sounds].sort((a, b) => at(a) - at(b) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}
