import { getPrefs, type Sound } from "@/lib/prefs";

/**
 * fuwa's sounds, made on the spot with Web Audio instead of shipped as files:
 * a soft two-note blip for a message, a brighter chime for a mention, and a
 * rising sparkle when someone joins.
 */

const TUNES: Record<Sound, { notes: number[]; step: number; length: number; type: OscillatorType }> = {
  message: { notes: [880, 1174.66], step: 0.07, length: 0.16, type: "sine" },
  mention: { notes: [987.77, 1318.51, 1760], step: 0.075, length: 0.22, type: "triangle" },
  join: { notes: [659.25, 830.61, 987.77, 1318.51], step: 0.06, length: 0.18, type: "sine" },
};

let context: AudioContext | null = null;

/** Plays a sound at the set volume. `force` plays it even when that sound is switched off, for previews. */
export function play(sound: Sound, force = false) {
  const p = getPrefs();
  if (!force && (!p.sounds[sound] || (p.streamer && p.streamerMuteSounds))) return;
  const volume = p.volume / 100;
  if (volume <= 0) return;
  try {
    context ??= new AudioContext();
    if (context.state === "suspended") void context.resume();
    const now = context.currentTime;
    const tune = TUNES[sound];
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
