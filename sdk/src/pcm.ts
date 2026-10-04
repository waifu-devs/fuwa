// Raw sound (16-bit PCM) for speech services that take or make it. Voice
// channels carry Opus, so turning PCM into Opus and back needs a codec: the
// SDK takes any Opus library through these two small interfaces and ships
// none itself.

/**
 * Turns 20 ms of PCM into one Opus packet. Make it with your Opus library at
 * the same sample rate and channel count you give `speakPcm`, for example
 * with `@discordjs/opus`:
 *
 *     const enc = new OpusEncoder(24_000, 1);
 *     const encoder = { encode: (pcm) => enc.encode(Buffer.from(pcm.buffer, pcm.byteOffset, pcm.byteLength)) };
 */
export interface OpusEncoder {
  encode(pcm: Int16Array): Uint8Array;
}

/**
 * Turns one Opus packet into PCM (interleaved when stereo), at whatever rate
 * your Opus library was made for: Opus decodes to 8, 12, 16, 24 or 48 kHz.
 */
export interface OpusDecoder {
  decode(opus: Uint8Array): Int16Array;
}

/** The rates Opus takes sound at. */
export type OpusSampleRate = 8000 | 12000 | 16000 | 24000 | 48000;

export interface PcmFormat {
  /** Default 48000. */
  sampleRate?: OpusSampleRate;
  /** Default 1 (mono); stereo is interleaved. */
  channels?: 1 | 2;
}

/**
 * Cuts PCM arriving in pieces of any length into 20 ms frames, as an Opus
 * encoder takes them. The last frame is padded with silence.
 */
export async function* pcmFrames(
  source: Iterable<Int16Array> | AsyncIterable<Int16Array>,
  format: PcmFormat = {},
): AsyncGenerator<Int16Array> {
  const rate = format.sampleRate ?? 48_000;
  if (![8000, 12000, 16000, 24000, 48000].includes(rate)) {
    throw new TypeError(`Opus takes sound at 8, 12, 16, 24 or 48 kHz, not ${rate} Hz`);
  }
  const size = (rate / 50) * (format.channels ?? 1);
  let frame = new Int16Array(size);
  let filled = 0;
  for await (const piece of source) {
    let at = 0;
    while (at < piece.length) {
      const n = Math.min(size - filled, piece.length - at);
      frame.set(piece.subarray(at, at + n), filled);
      filled += n;
      at += n;
      if (filled === size) {
        yield frame;
        frame = new Int16Array(size);
        filled = 0;
      }
    }
  }
  if (filled > 0) yield frame; // the rest is already zeros
}

/**
 * Little-endian 16-bit PCM bytes (what speech services send as "pcm" or
 * "s16le"), arriving in pieces of any size, as samples.
 */
export async function* pcmFromBytes(
  source: AsyncIterable<Uint8Array> | Iterable<Uint8Array>,
): AsyncGenerator<Int16Array> {
  let odd: number | undefined;
  for await (const chunk of source) {
    let bytes = chunk;
    if (odd !== undefined) {
      bytes = new Uint8Array(chunk.length + 1);
      bytes[0] = odd;
      bytes.set(chunk, 1);
      odd = undefined;
    }
    const whole = bytes.length & ~1;
    if (whole < bytes.length) odd = bytes[whole];
    if (whole === 0) continue;
    const view = new DataView(bytes.buffer, bytes.byteOffset, whole);
    const out = new Int16Array(whole / 2);
    for (let i = 0; i < out.length; i++) out[i] = view.getInt16(i * 2, true);
    yield out;
  }
}

/** Samples as little-endian 16-bit PCM bytes, for services that take raw PCM. */
export function pcmToBytes(pcm: Int16Array): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(pcm.length * 2);
  const view = new DataView(out.buffer);
  for (let i = 0; i < pcm.length; i++) view.setInt16(i * 2, pcm[i]!, true);
  return out;
}
