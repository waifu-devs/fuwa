import { readOggOpus } from "./ogg";

/**
 * For browsers that can't play Ogg Opus in an audio element: the same
 * sound decoded with WebCodecs into a WAV file, which every browser plays.
 */

/** Whether an audio element here plays Ogg Opus by itself. */
export function playsOgg(): boolean {
  if (typeof document === "undefined") return false;
  const a = document.createElement("audio");
  return a.canPlayType('audio/ogg; codecs="opus"') !== "";
}

/**
 * The most this fallback decodes: fifteen minutes. A file's own stated
 * length is what someone else wrote, so it only ever shortens this.
 */
const MAX_FRAMES = 15 * 60 * 48_000;

export async function oggToWav(ogg: Uint8Array): Promise<Uint8Array<ArrayBuffer>> {
  if (typeof AudioDecoder === "undefined") throw new Error("this browser can't play voice messages");
  const file = readOggOpus(ogg);
  const channels = Math.max(1, Math.min(2, file.channels));
  const limit = Math.min(MAX_FRAMES, file.samples > 0 ? file.preSkip + file.samples : MAX_FRAMES);
  // Recordings here are 20 ms packets, so the stated length never needs
  // more than this many; a file of tiny packets can't make it decode more.
  const packets = file.packets.slice(0, Math.ceil(limit / 960) + 1);
  const pcm: Float32Array[] = [];
  let kept = 0;
  let failed: unknown = null;
  const decoder = new AudioDecoder({
    output: (data) => {
      const frames = Math.min(data.numberOfFrames, limit - kept);
      if (frames <= 0) {
        data.close();
        return;
      }
      kept += frames;
      const out = new Float32Array(frames * channels);
      for (let c = 0; c < channels; c++) {
        const plane = new Float32Array(data.numberOfFrames);
        data.copyTo(plane, { planeIndex: c, format: "f32-planar" });
        for (let i = 0; i < frames; i++) out[i * channels + c] = plane[i]!;
      }
      pcm.push(out);
      data.close();
    },
    error: (err) => {
      failed = err;
    },
  });
  decoder.configure({ codec: "opus", sampleRate: 48_000, numberOfChannels: channels });
  let timestamp = 0;
  for (const packet of packets) {
    decoder.decode(new EncodedAudioChunk({ type: "key", timestamp, data: packet }));
    timestamp += 20_000;
  }
  await decoder.flush();
  decoder.close();
  if (failed) throw new Error("that voice message couldn't be played");
  // Drop the pre-skip, and anything past the stated length.
  const all = concat(pcm);
  const start = Math.min(all.length, file.preSkip * channels);
  const end = file.samples ? Math.min(all.length, start + file.samples * channels) : all.length;
  return wav(all.subarray(start, end), channels, 48_000);
}

function concat(parts: Float32Array[]): Float32Array {
  const out = new Float32Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

/** 16-bit PCM WAV. */
function wav(samples: Float32Array, channels: number, rate: number): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(44 + samples.length * 2);
  const v = new DataView(out.buffer);
  const text = (at: number, s: string) => [...s].forEach((ch, i) => (out[at + i] = ch.charCodeAt(0)));
  text(0, "RIFF");
  v.setUint32(4, 36 + samples.length * 2, true);
  text(8, "WAVE");
  text(12, "fmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, channels, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, rate * channels * 2, true);
  v.setUint16(32, channels * 2, true);
  v.setUint16(34, 16, true);
  text(36, "data");
  v.setUint32(40, samples.length * 2, true);
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]!));
    v.setInt16(44 + i * 2, s < 0 ? s * 0x8000 : s * 0x7fff, true);
  }
  return out;
}
