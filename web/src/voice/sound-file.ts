import { i18n, type Key } from "@/i18n/i18n";
import { FADE_FRAMES, framesToMs, MAX_SOUND_BYTES, SOUND_RATE, trimPackets, windowFrames } from "@/lib/soundboard";
import { writeOggOpus } from "./ogg";

/**
 * Making a soundboard sound on this device (docs/soundboard.md): any audio
 * file the browser can decode, read at 48 kHz through WebAudio, cut to the
 * part chosen (at most 10 seconds), encoded to Opus by the browser
 * (WebCodecs) and written as Ogg here, as voice messages are. Nothing
 * leaves the device until it's uploaded.
 */

/** Plenty for short sounds, and well under the instance's 1 MiB: 10 s of stereo is about 120 KB. */
const BITRATE = { 1: 64_000, 2: 96_000 } as const;
/** What libopus drops from the start when nothing says otherwise. */
const DEFAULT_PRE_SKIP = 312;
/** WebCodecs takes sound in blocks; 20 ms, like the packets. */
const BLOCK = 960;
/** The biggest file this reads, so a huge video doesn't fill memory. */
const MAX_INPUT_BYTES = 200 * 1024 * 1024;

const tr = (key: Key) => i18n().t(key);

/** Whether this browser can make sounds: it needs an Opus encoder. */
export const canMakeSounds = () => typeof AudioEncoder !== "undefined" && typeof OfflineAudioContext !== "undefined";

/** A file, decoded at 48 kHz, ready to cut. Throws a sentence when the browser can't read it. */
export async function decodeSoundFile(file: Blob): Promise<AudioBuffer> {
  if (file.size > MAX_INPUT_BYTES) throw new Error(tr("serversettings.soundboard.notAudio"));
  const bytes = await file.arrayBuffer();
  // An offline context decodes straight to its own rate, so the file comes out at 48 kHz.
  const ctx = new OfflineAudioContext(1, 1, SOUND_RATE);
  try {
    const buffer = await ctx.decodeAudioData(bytes);
    if (!buffer.length) throw new Error("empty");
    return buffer;
  } catch {
    throw new Error(tr("serversettings.soundboard.notAudio"));
  }
}

/** How long a decoded file plays, in milliseconds. */
export const lengthMs = (buffer: AudioBuffer) => (buffer.length / buffer.sampleRate) * 1000;

/** The window's samples, one array per channel (mono or stereo), with a short fade at each end. */
function cut(buffer: AudioBuffer, startMs: number, endMs: number): Float32Array[] {
  const { from, frames } = windowFrames(startMs, endMs, buffer.sampleRate);
  const take = Math.max(0, Math.min(frames, buffer.length - from));
  const channels = Math.min(2, buffer.numberOfChannels);
  const planes: Float32Array[] = [];
  for (let c = 0; c < channels; c++) {
    const plane = buffer.getChannelData(c).slice(from, from + take);
    const fade = Math.min(FADE_FRAMES, Math.floor(take / 2));
    for (let i = 0; i < fade; i++) {
      const g = i / fade;
      plane[i] = plane[i]! * g;
      plane[take - 1 - i] = plane[take - 1 - i]! * g;
    }
    planes.push(plane);
  }
  // Two copies of the same sound are a mono one.
  if (planes.length === 2 && planes[0]!.every((x, i) => x === planes[1]![i])) planes.pop();
  return planes;
}

/** A cut of the file, as Ogg Opus, with how long it plays. */
export async function makeSound(buffer: AudioBuffer, startMs: number, endMs: number): Promise<{ ogg: Uint8Array<ArrayBuffer>; durationMs: number }> {
  if (!canMakeSounds()) throw new Error(tr("serversettings.soundboard.cantEncode"));
  let planes = cut(buffer, startMs, endMs);
  const frames = planes[0]?.length ?? 0;
  if (!frames) throw new Error(tr("serversettings.soundboard.notAudio"));
  let config: AudioEncoderConfig = { codec: "opus", sampleRate: SOUND_RATE, numberOfChannels: planes.length, bitrate: BITRATE[planes.length as 1 | 2] };
  if (planes.length === 2 && !(await AudioEncoder.isConfigSupported(config)).supported) {
    // No stereo here: mixed down.
    const [l, r] = planes as [Float32Array, Float32Array];
    planes = [l.map((x, i) => (x + r[i]!) / 2)];
    config = { ...config, numberOfChannels: 1, bitrate: BITRATE[1] };
  }
  if (!(await AudioEncoder.isConfigSupported(config)).supported) throw new Error(tr("serversettings.soundboard.cantEncode"));

  const packets: { data: Uint8Array; samples: number }[] = [];
  let preSkip = DEFAULT_PRE_SKIP;
  let failed: unknown = null;
  const encoder = new AudioEncoder({
    output: (chunk, meta) => {
      const description = meta?.decoderConfig?.description;
      if (description && packets.length === 0) {
        const head = ArrayBuffer.isView(description)
          ? new Uint8Array(description.buffer, description.byteOffset, description.byteLength)
          : new Uint8Array(description);
        // An OpusHead says how much the decoder skips.
        if (head.length >= 12 && String.fromCharCode(...head.subarray(0, 8)) === "OpusHead") preSkip = head[10]! | (head[11]! << 8);
      }
      const data = new Uint8Array(chunk.byteLength);
      chunk.copyTo(data);
      packets.push({ data, samples: chunk.duration ? Math.round((chunk.duration * SOUND_RATE) / 1_000_000) : BLOCK });
    },
    error: (err) => {
      failed = err;
    },
  });
  try {
    encoder.configure(config);
    const channels = planes.length;
    for (let at = 0; at < frames; at += BLOCK) {
      const n = Math.min(BLOCK, frames - at);
      // Interleaved, as "f32" takes it.
      const block = new Float32Array(n * channels);
      for (let c = 0; c < channels; c++) {
        const plane = planes[c]!;
        for (let i = 0; i < n; i++) block[i * channels + c] = plane[at + i]!;
      }
      const data = new AudioData({ format: "f32", sampleRate: SOUND_RATE, numberOfFrames: n, numberOfChannels: channels, timestamp: Math.round((at / SOUND_RATE) * 1_000_000), data: block });
      encoder.encode(data);
      data.close();
    }
    await encoder.flush();
  } catch (err) {
    failed ??= err;
  } finally {
    if (encoder.state !== "closed") encoder.close();
  }
  if (failed) throw new Error(tr("serversettings.soundboard.cantEncode"));
  const samples = trimPackets(
    packets.map((p) => p.samples),
    frames,
    preSkip,
  );
  const ogg = writeOggOpus(
    samples.map((s, i) => ({ data: packets[i]!.data, samples: s })),
    planes.length,
    preSkip,
  ) as Uint8Array<ArrayBuffer>;
  if (ogg.length > MAX_SOUND_BYTES) throw new Error(tr("serversettings.soundboard.tooBig"));
  return { ogg, durationMs: framesToMs(frames) };
}
