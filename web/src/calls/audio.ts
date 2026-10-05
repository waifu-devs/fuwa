import { i18n, type Key } from "@/i18n/i18n";
import { getPrefs, subscribePrefs, type Prefs } from "@/lib/prefs";

/** A sentence in the app's language now. */
const tr = (key: Key) => i18n().t(key);

/**
 * Sound in and out of a call, through Web Audio.
 *
 * In: the microphone, through your input volume, then a gate that opens
 * while you talk (voice activity) or hold the key (push to talk), and stays
 * shut while you're muted. What leaves the gate is the one track a call
 * sends, so changing microphones never touches the connection.
 *
 * Out: each person's sound through their own volume (yours to set), then
 * your call volume, to the output device you picked. Deafened, it all goes
 * quiet. Every voice also passes a meter, so the app can show who's talking.
 */

/** Below this (in dB), nobody's talking: it's a room's hum. */
const QUIET = -100;
/** Voice activity holds the gate open this long after the last loud moment, so words don't clip. */
const HANGOVER_MS = 320;
/** Someone else counts as talking above this level (after their own processing). */
const SPEAKING_DB = -52;

let context: AudioContext | null = null;

/** The one audio context calls use, made on the first call (browsers only allow it after a click). */
export function audioContext(): AudioContext {
  context ??= new AudioContext({ latencyHint: "interactive" });
  if (context.state === "suspended") void context.resume();
  return context;
}

/** The loudness of what an analyser hears right now, in dB (QUIET for silence). */
export function levelOf(analyser: AnalyserNode, buffer: Float32Array<ArrayBuffer>): number {
  analyser.getFloatTimeDomainData(buffer);
  let sum = 0;
  for (const x of buffer) sum += x * x;
  const rms = Math.sqrt(sum / buffer.length);
  return rms > 0 ? Math.max(QUIET, 20 * Math.log10(rms)) : QUIET;
}

const constraints = (p: Prefs): MediaTrackConstraints => ({
  deviceId: p.inputDevice ? { exact: p.inputDevice } : undefined,
  echoCancellation: p.echoCancellation,
  noiseSuppression: p.noiseSuppression,
  autoGainControl: p.autoGainControl,
  channelCount: 1,
});

/** Why the microphone didn't open, in words. */
export function micProblem(err: unknown): string {
  const name = err instanceof DOMException ? err.name : "";
  if (name === "NotAllowedError" || name === "SecurityError") return tr("workspace.calls.mic.blocked");
  if (name === "NotFoundError" || name === "OverconstrainedError") return tr("workspace.calls.mic.notFound");
  if (name === "NotReadableError") return tr("workspace.calls.mic.busy");
  if (typeof navigator !== "undefined" && !navigator.mediaDevices) return tr("workspace.calls.mic.noHttps");
  return tr("workspace.calls.mic.failed");
}

export type MicState = {
  /** The level going in, in dB, before the gate. */
  level: number;
  /** The level voice activity opens at now, in dB. */
  threshold: number;
  /** The gate is open: what you say goes out. */
  open: boolean;
};

/**
 * Your microphone, ready for a call. `muted` and `pushing` are read on every
 * tick, so they change without reopening anything.
 */
export class Mic {
  readonly ctx = audioContext();
  private stream: MediaStream | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private readonly input = this.ctx.createGain();
  private readonly analyser = this.ctx.createAnalyser();
  private readonly gate = this.ctx.createGain();
  private readonly out = this.ctx.createMediaStreamDestination();
  private readonly buffer: Float32Array<ArrayBuffer>;
  private timer: ReturnType<typeof setInterval> | null = null;
  private lastLoud = 0;
  private releaseAt = 0;
  private floor = -60;
  private wasPushing = false;
  private opened = false;
  private unsubscribe: () => void;
  private applied: Prefs;
  /** The track a call sends. */
  readonly track: MediaStreamTrack;
  muted = false;
  pushing = false;

  private constructor(private readonly onTick: (state: MicState) => void) {
    this.analyser.fftSize = 1024;
    this.analyser.smoothingTimeConstant = 0.2;
    this.buffer = new Float32Array(this.analyser.fftSize);
    this.input.connect(this.analyser);
    this.input.connect(this.gate);
    this.gate.connect(this.out);
    this.gate.gain.value = 0;
    this.track = this.out.stream.getAudioTracks()[0]!;
    this.applied = getPrefs();
    this.input.gain.value = this.applied.inputVolume / 100;
    this.unsubscribe = subscribePrefs(() => void this.follow(getPrefs()));
  }

  /** Opens the microphone. Throws what getUserMedia threw when it can't. */
  static async open(onTick: (state: MicState) => void): Promise<Mic> {
    const mic = new Mic(onTick);
    try {
      await mic.useDevice(getPrefs());
    } catch (err) {
      mic.close();
      throw err;
    }
    mic.timer = setInterval(() => mic.tick(), 30);
    return mic;
  }

  private async useDevice(p: Prefs) {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: constraints(p) });
    const source = this.ctx.createMediaStreamSource(stream);
    source.connect(this.input);
    this.source?.disconnect();
    this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = stream;
    this.source = source;
  }

  /** Picks up changed settings: a new microphone or processing reopens it; the rest applies at once. */
  private async follow(p: Prefs) {
    const before = this.applied;
    this.applied = p;
    this.input.gain.setTargetAtTime(p.inputVolume / 100, this.ctx.currentTime, 0.02);
    const reopen =
      before.inputDevice !== p.inputDevice ||
      before.echoCancellation !== p.echoCancellation ||
      before.noiseSuppression !== p.noiseSuppression ||
      before.autoGainControl !== p.autoGainControl;
    if (reopen && this.stream) await this.useDevice(p).catch(() => {});
  }

  private tick() {
    const p = this.applied;
    const level = levelOf(this.analyser, this.buffer);
    const now = performance.now();
    // The room's hum, followed slowly up and quickly down, for automatic sensitivity.
    this.floor = level < this.floor ? this.floor + (level - this.floor) * 0.3 : this.floor + (level - this.floor) * 0.005;
    const threshold = p.autoSensitivity ? Math.min(-28, Math.max(-70, this.floor + 14)) : p.sensitivity;
    let open: boolean;
    if (p.inputMode === "ptt") {
      if (this.wasPushing && !this.pushing) this.releaseAt = now + p.pttRelease;
      this.wasPushing = this.pushing;
      open = this.pushing || now < this.releaseAt;
    } else {
      if (level > threshold) this.lastLoud = now;
      open = now - this.lastLoud < HANGOVER_MS;
    }
    open &&= !this.muted;
    if (open !== this.opened) {
      this.opened = open;
      // Quick to open, so the first syllable gets through; a little softer to close.
      this.gate.gain.setTargetAtTime(open ? 1 : 0, this.ctx.currentTime, open ? 0.005 : 0.04);
    }
    this.onTick({ level, threshold, open });
  }

  close() {
    if (this.timer) clearInterval(this.timer);
    this.unsubscribe();
    this.source?.disconnect();
    this.stream?.getTracks().forEach((t) => t.stop());
    this.track.stop();
    this.gate.disconnect();
    this.input.disconnect();
  }
}

type Voice = { stream: MediaStream; element: HTMLAudioElement; source: MediaStreamAudioSourceNode; gain: GainNode; analyser: AnalyserNode; buffer: Float32Array<ArrayBuffer>; talking: boolean; lastLoud: number };

/**
 * Everyone else's sound in a call, by user id. `volumeOf` says how loud
 * someone should be (0 to 2); `onSpeaking` hears who starts and stops talking.
 */
export class Speakers {
  readonly ctx = audioContext();
  private readonly master = this.ctx.createGain();
  private readonly voices = new Map<string, Voice>();
  private timer: ReturnType<typeof setInterval>;
  private unsubscribe: () => void;
  private sink = "";
  /** While recording: where everyone's sound (and yours) also goes. */
  private tap: { gain: GainNode; out: MediaStreamAudioDestinationNode; mine: MediaStreamAudioSourceNode | null } | null = null;
  deafened = false;

  constructor(
    private readonly volumeOf: (userId: string) => number,
    private readonly onSpeaking: (userId: string, talking: boolean) => void,
  ) {
    this.master.connect(this.ctx.destination);
    this.apply();
    this.unsubscribe = subscribePrefs(() => this.apply());
    this.timer = setInterval(() => this.tick(), 60);
  }

  /** Someone's sound arrived (or came back on a new connection). */
  add(userId: string, stream: MediaStream) {
    this.remove(userId);
    // Chrome only plays a remote track through Web Audio while an element holds it too.
    const element = new Audio();
    element.muted = true;
    element.srcObject = stream;
    void element.play().catch(() => {});
    const source = this.ctx.createMediaStreamSource(stream);
    const gain = this.ctx.createGain();
    const analyser = this.ctx.createAnalyser();
    analyser.fftSize = 512;
    gain.gain.value = this.volumeOf(userId);
    source.connect(gain).connect(analyser).connect(this.master);
    if (this.tap) analyser.connect(this.tap.gain);
    this.voices.set(userId, { stream, element, source, gain, analyser, buffer: new Float32Array(analyser.fftSize), talking: false, lastLoud: 0 });
  }

  remove(userId: string) {
    const v = this.voices.get(userId);
    if (!v) return;
    this.voices.delete(userId);
    v.source.disconnect();
    v.gain.disconnect();
    v.analyser.disconnect();
    v.element.srcObject = null;
    if (v.talking) this.onSpeaking(userId, false);
  }

  /** Volumes, deafening and the output device, as the settings say now. */
  apply() {
    const p = getPrefs();
    this.master.gain.setTargetAtTime(this.deafened ? 0 : p.outputVolume / 100, this.ctx.currentTime, 0.03);
    for (const [userId, v] of this.voices) v.gain.gain.setTargetAtTime(this.volumeOf(userId), this.ctx.currentTime, 0.03);
    if (p.outputDevice !== this.sink) {
      this.sink = p.outputDevice;
      const ctx = this.ctx as AudioContext & { setSinkId?: (id: string) => Promise<void> };
      void ctx.setSinkId?.(p.outputDevice).catch(() => {});
    }
  }

  private tick() {
    const now = performance.now();
    for (const [userId, v] of this.voices) {
      if (levelOf(v.analyser, v.buffer) > SPEAKING_DB) v.lastLoud = now;
      const talking = now - v.lastLoud < HANGOVER_MS;
      if (talking !== v.talking) {
        v.talking = talking;
        this.onSpeaking(userId, talking);
      }
    }
  }

  /**
   * Everyone's sound as you hear it (at the volumes you gave them, but not
   * deafened or turned down overall), with `mine` (your microphone, as it
   * goes out) mixed in, as one stream to record. Anyone who joins later
   * joins the mix.
   */
  record(mine: MediaStreamTrack | null): MediaStream {
    this.stopRecording();
    const gain = this.ctx.createGain();
    const out = this.ctx.createMediaStreamDestination();
    gain.connect(out);
    for (const v of this.voices.values()) v.analyser.connect(gain);
    const source = mine ? this.ctx.createMediaStreamSource(new MediaStream([mine])) : null;
    source?.connect(gain);
    this.tap = { gain, out, mine: source };
    return out.stream;
  }

  stopRecording() {
    if (!this.tap) return;
    for (const v of this.voices.values()) v.analyser.disconnect(this.tap.gain);
    this.tap.mine?.disconnect();
    this.tap.gain.disconnect();
    this.tap = null;
  }

  close() {
    clearInterval(this.timer);
    this.unsubscribe();
    this.stopRecording();
    for (const id of [...this.voices.keys()]) this.remove(id);
    this.master.disconnect();
  }
}

/** Whether this browser can send sound to a device other than the default one. */
export const canPickOutput = () => typeof AudioContext !== "undefined" && "setSinkId" in AudioContext.prototype;
