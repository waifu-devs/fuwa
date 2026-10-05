import { i18n } from "@/i18n/i18n";
import { getPrefs } from "@/lib/prefs";
import { reportError, reportUsage } from "@/lib/reports";
import captureUrl from "./capture.worklet.ts?worker&url";
import { writeOggOpus } from "./ogg";
import { waveform } from "./waveform";

/**
 * Recording a voice message on this device: the microphone's sound,
 * encoded to Opus by the browser (WebCodecs) and written as an Ogg file
 * here, with its waveform. Nothing leaves the device until it's sent, and
 * then only sealed (see seal.ts).
 */

/** A finished recording. */
export type Clip = { ogg: Uint8Array<ArrayBuffer>; durationMs: number; waveform: Uint8Array<ArrayBuffer> };

const RATE = 48_000;
/** Samples a level is taken over: 20 ms. */
const BLOCK = 960;
/** Plenty for a voice: a minute is about 240 KB. */
const BITRATE = 32_000;
/** What libopus drops from the start when nothing says otherwise. */
const DEFAULT_PRE_SKIP = 312;
/** Shorter than this isn't worth sending: a tap, not a message. */
export const MIN_MS = 500;

/** Whether this browser can record voice messages. */
export function canRecord(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof AudioEncoder !== "undefined" &&
    typeof AudioWorkletNode !== "undefined" &&
    !!navigator.mediaDevices?.getUserMedia
  );
}

/** Turns sound at one rate into 48 kHz, carrying its place across blocks. */
class Resampler {
  private pos = 0;
  private last = 0;
  constructor(private readonly from: number) {}
  push(input: Float32Array): Float32Array {
    if (this.from === RATE) return input;
    const step = this.from / RATE;
    const out: number[] = [];
    // Positions are in input samples, -1 being the last sample of the block before.
    while (this.pos < input.length - 1) {
      const i = Math.floor(this.pos);
      const a = i < 0 ? this.last : input[i]!;
      const b = input[i + 1]!;
      out.push(a + (b - a) * (this.pos - i));
      this.pos += step;
    }
    this.pos -= input.length;
    this.last = input[input.length - 1] ?? 0;
    return Float32Array.from(out);
  }
}

export type RecorderEvents = {
  /** The latest level, 0 to 1, about 25 times a second. */
  onLevel?: (level: number) => void;
  /** It reached the instance's longest voice message and stopped listening. */
  onLimit?: () => void;
};

export class Recorder {
  private packets: { data: Uint8Array; samples: number }[] = [];
  private levels: number[] = [];
  private block = 0;
  private blockPeak = 0;
  private samples = 0;
  private preSkip = DEFAULT_PRE_SKIP;
  private failed: unknown = null;
  private done = false;

  private constructor(
    private readonly stream: MediaStream,
    private readonly ctx: AudioContext,
    private readonly node: AudioWorkletNode,
    private readonly encoder: AudioEncoder,
    private readonly maxSamples: number,
    private readonly events: RecorderEvents,
  ) {}

  /**
   * Opens the microphone and starts recording, up to `maxMs` (0 for no cap).
   * Throws what getUserMedia threw when the microphone can't open.
   */
  static async start(maxMs: number, events: RecorderEvents = {}): Promise<Recorder> {
    if (!canRecord()) throw new DOMException("No recording here", "NotSupportedError");
    const p = getPrefs();
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        deviceId: p.inputDevice ? { ideal: p.inputDevice } : undefined,
        echoCancellation: p.echoCancellation,
        noiseSuppression: p.noiseSuppression,
        autoGainControl: p.autoGainControl,
        channelCount: 1,
      },
    });
    const stopTracks = () => stream.getTracks().forEach((t) => t.stop());
    let ctx: AudioContext | null = null;
    try {
      const config: AudioEncoderConfig = { codec: "opus", sampleRate: RATE, numberOfChannels: 1, bitrate: BITRATE };
      if (!(await AudioEncoder.isConfigSupported(config)).supported) throw new DOMException("No Opus encoder", "NotSupportedError");
      ctx = new AudioContext();
      await ctx.audioWorklet.addModule(captureUrl);
      const source = ctx.createMediaStreamSource(stream);
      const node = new AudioWorkletNode(ctx, "fuwa-voice-capture", { numberOfOutputs: 1 });
      // Muted, but connected, so the browser keeps pulling sound through it.
      const mute = ctx.createGain();
      mute.gain.value = 0;
      source.connect(node).connect(mute).connect(ctx.destination);
      let recorder: Recorder | null = null;
      const encoder = new AudioEncoder({
        output: (chunk, meta) => recorder?.take(chunk, meta),
        error: (err) => {
          if (recorder) recorder.failed = err;
        },
      });
      encoder.configure(config);
      const maxSamples = maxMs > 0 ? Math.floor((maxMs / 1000) * RATE) : 0;
      recorder = new Recorder(stream, ctx, node, encoder, maxSamples, events);
      const resample = new Resampler(ctx.sampleRate);
      node.port.onmessage = (e: MessageEvent<Float32Array>) => recorder?.feed(resample.push(e.data));
      if (ctx.state === "suspended") await ctx.resume();
      reportUsage("voice.record");
      return recorder;
    } catch (err) {
      stopTracks();
      void ctx?.close();
      throw err;
    }
  }

  /** How long it's recorded so far. */
  get elapsedMs() {
    return (this.samples / RATE) * 1000;
  }

  /** Levels so far, one every 20 ms, 0 to 1. */
  get recentLevels(): readonly number[] {
    return this.levels;
  }

  private take(chunk: EncodedAudioChunk, meta?: EncodedAudioChunkMetadata) {
    const description = meta?.decoderConfig?.description;
    if (description && this.packets.length === 0) {
      const bytes = ArrayBuffer.isView(description)
        ? new Uint8Array(description.buffer, description.byteOffset, description.byteLength)
        : new Uint8Array(description);
      // An OpusHead says how much the decoder skips.
      if (bytes.length >= 12 && String.fromCharCode(...bytes.subarray(0, 8)) === "OpusHead") {
        this.preSkip = bytes[10]! | (bytes[11]! << 8);
      }
    }
    const data = new Uint8Array(chunk.byteLength);
    chunk.copyTo(data);
    const samples = chunk.duration ? Math.round((chunk.duration * RATE) / 1_000_000) : BLOCK;
    this.packets.push({ data, samples });
  }

  private feed(input: Float32Array) {
    if (this.done || !input.length) return;
    let take = input;
    if (this.maxSamples && this.samples + take.length >= this.maxSamples) {
      take = input.subarray(0, Math.max(0, this.maxSamples - this.samples));
    }
    for (const s of take) {
      const a = Math.abs(s);
      if (a > this.blockPeak) this.blockPeak = a;
      if (++this.block === BLOCK) {
        this.levels.push(this.blockPeak);
        this.events.onLevel?.(this.blockPeak);
        this.block = 0;
        this.blockPeak = 0;
      }
    }
    if (take.length) {
      const data = new AudioData({
        format: "f32",
        sampleRate: RATE,
        numberOfFrames: take.length,
        numberOfChannels: 1,
        timestamp: Math.round((this.samples / RATE) * 1_000_000),
        data: Float32Array.from(take),
      });
      this.samples += take.length;
      try {
        this.encoder.encode(data);
      } catch (err) {
        this.failed = err;
      }
      data.close();
    }
    if (this.maxSamples && this.samples >= this.maxSamples) {
      this.release();
      this.events.onLimit?.();
    }
  }

  /** Stops listening and lets the microphone go. */
  private release() {
    if (this.done) return;
    this.done = true;
    this.node.port.onmessage = null;
    this.node.disconnect();
    this.stream.getTracks().forEach((t) => t.stop());
    void this.ctx.close().catch(() => {});
  }

  /** Stops and answers the recording. */
  async stop(): Promise<Clip> {
    this.release();
    try {
      if (this.encoder.state === "configured") await this.encoder.flush();
    } catch (err) {
      this.failed ??= err;
    }
    if (this.encoder.state !== "closed") this.encoder.close();
    if (this.failed) {
      reportError("voice_encode", "voice.record");
      throw new Error(i18n().t("system.voice.notSaved"));
    }
    const durationMs = Math.round(this.elapsedMs);
    return {
      ogg: writeOggOpus(this.packets, 1, this.preSkip) as Uint8Array<ArrayBuffer>,
      durationMs,
      waveform: waveform(this.levels) as Uint8Array<ArrayBuffer>,
    };
  }

  /** Throws the recording away. */
  cancel() {
    this.release();
    if (this.encoder.state !== "closed") this.encoder.close();
    this.packets = [];
  }
}
