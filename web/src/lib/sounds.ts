import { getPrefs, type Sound } from "@/lib/prefs";

/**
 * fuwa's sounds, made on the spot with Web Audio instead of shipped as files:
 * a soft two-note blip for a message, a brighter chime for a mention, and a
 * rising sparkle when someone joins.
 */

type Tune = { notes: number[]; step: number; length: number; type: OscillatorType };

const TUNES: Record<Sound, Tune> = {
  message: { notes: [880, 1174.66], step: 0.07, length: 0.16, type: "sine" },
  mention: { notes: [987.77, 1318.51, 1760], step: 0.075, length: 0.22, type: "triangle" },
  join: { notes: [659.25, 830.61, 987.77, 1318.51], step: 0.06, length: 0.18, type: "sine" },
  call: { notes: [587.33, 880], step: 0.08, length: 0.2, type: "sine" },
  ring: { notes: [783.99, 987.77, 1174.66, 987.77, 1174.66], step: 0.11, length: 0.3, type: "triangle" },
};

/** What happens in a call, each with its own little tune: up for coming in or switching on, down for going. */
export type CallCue = "connect" | "disconnect" | "someoneJoined" | "someoneLeft" | "mute" | "unmute" | "deafen" | "undeafen" | "recording";

const CUES: Record<CallCue, Tune> = {
  connect: { notes: [523.25, 659.25, 783.99, 1046.5], step: 0.07, length: 0.2, type: "sine" },
  disconnect: { notes: [783.99, 659.25, 523.25], step: 0.08, length: 0.22, type: "sine" },
  someoneJoined: { notes: [659.25, 987.77], step: 0.07, length: 0.18, type: "sine" },
  someoneLeft: { notes: [987.77, 659.25], step: 0.07, length: 0.18, type: "sine" },
  mute: { notes: [698.46, 523.25], step: 0.05, length: 0.12, type: "triangle" },
  unmute: { notes: [523.25, 698.46], step: 0.05, length: 0.12, type: "triangle" },
  deafen: { notes: [587.33, 440, 349.23], step: 0.05, length: 0.14, type: "triangle" },
  undeafen: { notes: [349.23, 440, 587.33], step: 0.05, length: 0.14, type: "triangle" },
  // Someone in the call started recording: two soft, even beeps, hard to miss.
  recording: { notes: [880, 880], step: 0.16, length: 0.1, type: "sine" },
};

/** Plays a call's cue, when call sounds are on. */
export function cue(which: CallCue) {
  const p = getPrefs();
  if (!p.sounds.call || (p.streamer && p.streamerMuteSounds)) return;
  tune(CUES[which], p.volume / 100);
}

let context: AudioContext | null = null;

/** Plays a sound at the set volume. `force` plays it even when that sound is switched off, for previews. */
export function play(sound: Sound, force = false) {
  const p = getPrefs();
  if (!force && (!p.sounds[sound] || (p.streamer && p.streamerMuteSounds))) return;
  const volume = p.volume / 100;
  if (volume <= 0) return;
  try {
    tune(TUNES[sound], volume);
  } catch {
    // No audio here (or not allowed yet): stay quiet.
  }
}

function tune(tune: Tune, volume: number) {
  if (volume <= 0) return;
  try {
    context ??= new AudioContext();
    if (context.state === "suspended") void context.resume();
    const now = context.currentTime;
    tune.notes.forEach((frequency, n) => {
      const start = now + n * tune.step;
      const osc = context!.createOscillator();
      const gain = context!.createGain();
      osc.type = tune.type;
      osc.frequency.setValueAtTime(frequency, start);
      gain.gain.setValueAtTime(0, start);
      gain.gain.linearRampToValueAtTime(0.18 * volume, start + 0.012);
      gain.gain.exponentialRampToValueAtTime(0.0001, start + tune.length);
      osc.connect(gain).connect(context!.destination);
      osc.start(start);
      osc.stop(start + tune.length + 0.02);
    });
  } catch {
    // No audio here (or not allowed yet): stay quiet.
  }
}

/**
 * The gacha's sounds (docs/economy.md), richer than the tunes above: each is
 * a few voices that can sweep from one pitch to another, or be a hiss of
 * noise, rising over `attack` seconds and dying away by `length`. The pack
 * charges up, bursts, each card flips with a tick, and its tier answers with
 * a chime that grows grander with rarity (`rarity` in lib/gacha.ts). The
 * desktop's `core/sounds.rs` plays the same voices.
 */
export type GachaCue = "charge" | "burst" | "flip" | "common" | "rare" | "epic" | "legendary" | "tap";

type Voice = { wave: OscillatorType | "noise"; from: number; to?: number; at: number; length: number; attack?: number; gain?: number };

const note = (wave: OscillatorType, from: number, at: number, length: number, gain = 1): Voice => ({ wave, from, at, length, gain });

export const GACHA_CUES: Record<GachaCue, Voice[]> = {
  charge: [
    { wave: "sine", from: 196, to: 784, at: 0, length: 1.3, attack: 1.15, gain: 0.9 },
    { wave: "triangle", from: 392, to: 1568, at: 0.05, length: 1.25, attack: 1.1, gain: 0.35 },
    { wave: "noise", from: 0, at: 0.2, length: 1.1, attack: 1, gain: 0.12 },
  ],
  burst: [
    { wave: "noise", from: 0, at: 0, length: 0.5, attack: 0.004, gain: 0.7 },
    { wave: "sine", from: 160, to: 45, at: 0, length: 0.4, attack: 0.004, gain: 1 },
    { wave: "triangle", from: 1046.5, to: 2093, at: 0.02, length: 0.25, attack: 0.01, gain: 0.3 },
  ],
  flip: [
    { wave: "noise", from: 0, at: 0, length: 0.07, attack: 0.002, gain: 0.35 },
    { wave: "triangle", from: 1800, to: 1100, at: 0, length: 0.06, attack: 0.002, gain: 0.3 },
  ],
  common: [note("sine", 659.25, 0.03, 0.18)],
  rare: [note("triangle", 783.99, 0.03, 0.22), note("triangle", 1174.66, 0.1, 0.26)],
  epic: [note("sine", 659.25, 0.03, 0.3), note("sine", 830.61, 0.1, 0.3), note("sine", 987.77, 0.17, 0.3), note("sine", 1318.51, 0.24, 0.4)],
  legendary: [
    ...[523.25, 659.25, 783.99, 1046.5, 1318.51, 1567.98].map((f, n) => note("triangle", f, 0.03 + n * 0.07, 0.45, 0.9)),
    note("sine", 2093, 0.45, 0.9, 0.35),
    note("sine", 2637.02, 0.52, 0.9, 0.3),
    { wave: "noise", from: 0, at: 0.4, length: 0.6, attack: 0.3, gain: 0.08 },
  ],
  tap: [note("sine", 1046.5, 0, 0.07, 0.6)],
};

/** The chime for a card of this rarity (0 to 3). */
export const RARITY_CUES: GachaCue[] = ["common", "rare", "epic", "legendary"];

let noise: AudioBuffer | null = null;

/** Plays one of the gacha's sounds, when they're on (the gacha's own switch, the volume and streamer mode). */
export function gachaCue(which: GachaCue) {
  const p = getPrefs();
  if (!p.gachaSounds || (p.streamer && p.streamerMuteSounds)) return;
  const volume = p.volume / 100;
  if (volume <= 0) return;
  try {
    context ??= new AudioContext();
    if (context.state === "suspended") void context.resume();
    const ctx = context;
    const now = ctx.currentTime;
    for (const v of GACHA_CUES[which]) {
      const start = now + v.at;
      const attack = v.attack ?? 0.012;
      const gain = ctx.createGain();
      gain.gain.setValueAtTime(0, start);
      gain.gain.linearRampToValueAtTime(0.18 * volume * (v.gain ?? 1), start + attack);
      gain.gain.exponentialRampToValueAtTime(0.0001, start + v.length);
      let source: AudioScheduledSourceNode;
      if (v.wave === "noise") {
        if (!noise) {
          noise = ctx.createBuffer(1, ctx.sampleRate, ctx.sampleRate);
          const data = noise.getChannelData(0);
          for (let i = 0; i < data.length; i++) data[i] = Math.random() * 2 - 1;
        }
        const buffer = ctx.createBufferSource();
        buffer.buffer = noise;
        buffer.loop = true;
        source = buffer;
      } else {
        const osc = ctx.createOscillator();
        osc.type = v.wave;
        osc.frequency.setValueAtTime(v.from, start);
        if (v.to) osc.frequency.exponentialRampToValueAtTime(v.to, start + v.length);
        source = osc;
      }
      source.connect(gain).connect(ctx.destination);
      source.start(start);
      source.stop(start + v.length + 0.02);
    }
  } catch {
    // No audio here (or not allowed yet): stay quiet.
  }
}
