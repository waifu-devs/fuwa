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
