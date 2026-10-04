/**
 * A voice message's waveform: its loudness over time as up to `BARS` bytes
 * (0 silence, 255 loudest), worked out on the sender's device and sent
 * inside the encrypted message, so the instance never learns its shape.
 */

/** How many values a waveform carries. */
export const BARS = 64;
/** The most a received one is read to. */
export const MAX_BARS = 128;

/** Peak levels (0 to 1), one per short block of sound, folded into `BARS` bytes. */
export function waveform(levels: number[], bars = BARS): Uint8Array {
  const out = new Uint8Array(Math.min(bars, Math.max(1, levels.length)));
  if (!levels.length) return out;
  const loudest = Math.max(0.02, ...levels);
  for (let i = 0; i < out.length; i++) {
    const from = Math.floor((i * levels.length) / out.length);
    const to = Math.max(from + 1, Math.floor(((i + 1) * levels.length) / out.length));
    let peak = 0;
    for (let j = from; j < to; j++) peak = Math.max(peak, levels[j]!);
    // Square root: quiet speech still shows, like ears hear it.
    out[i] = Math.round(Math.sqrt(Math.min(1, peak / loudest)) * 255);
  }
  return out;
}

/** A received waveform, as heights 0 to 1, `bars` long (stretched or squeezed to fit). */
export function heights(data: Uint8Array | undefined, bars: number): number[] {
  const src = data?.length ? data.subarray(0, MAX_BARS) : new Uint8Array(1);
  return Array.from({ length: bars }, (_, i) => {
    const from = Math.floor((i * src.length) / bars);
    const to = Math.max(from + 1, Math.floor(((i + 1) * src.length) / bars));
    let peak = 0;
    for (let j = from; j < to && j < src.length; j++) peak = Math.max(peak, src[j]!);
    return peak / 255;
  });
}
