import type { Fuwa } from "./client.js";
import { CanceledError, Code, FailedPreconditionError, FuwaError, UnavailableError, toFuwaError } from "./errors.js";
import type { ListenVoiceResponse } from "./gen/fuwa/v1/call_pb.js";
import type { VoiceState } from "./gen/fuwa/v1/types_pb.js";
import { OggOpusWriter, chunks, oggOpusPackets, opusPacketDuration, readOggOpus, splitOpusPacket } from "./ogg.js";
import { pcmFrames, type OpusDecoder, type OpusEncoder, type PcmFormat } from "./pcm.js";
import { sleep } from "./retry.js";

/** One frame of someone's sound. */
export interface VoiceFrame {
  /** Whose it is: their account id. */
  userId: string;
  /** One Opus packet, 20 ms at 48 kHz. Decode it with any Opus library. */
  opus: Uint8Array;
  /** When it was spoken, in 48 kHz ticks of their own clock (960 a frame); a bigger gap is lost sound or silence. */
  timestamp: number;
}

export interface JoinVoiceOptions {
  serverId: string;
  /** A voice channel the account may CONNECT to. */
  channelId: string;
  /** Join muted or deafened (shown to everyone, as for people). */
  selfMute?: boolean;
  selfDeaf?: boolean;
  /**
   * How long without sound from someone before they count as having
   * stopped speaking. Default 300 ms.
   */
  silenceMs?: number;
  /**
   * How long someone stays quiet before what they said counts as finished
   * (the end of an utterance, the cue to answer). Default 600 ms.
   */
  utteranceGapMs?: number;
  /** The longest an utterance runs before it ends and the next begins. Default 60 s. */
  maxUtteranceMs?: number;
  /**
   * How long without anything from the instance before the stream counts as
   * gone and it rejoins. Instances send a keepalive every 15 s while nobody
   * talks; this only applies once one has come (older instances send none).
   * Default 45 s.
   */
  keepaliveTimeoutMs?: number;
  /** Leaves when it aborts. */
  signal?: AbortSignal;
}

type Listener<A extends unknown[]> = (...args: A) => unknown;

export interface VoiceEvents {
  /** A frame of someone's sound. */
  frame: Listener<[VoiceFrame]>;
  /** Someone started sending sound. */
  speaking: Listener<[string]>;
  /** Someone stopped (no sound for `silenceMs`). */
  silent: Listener<[string]>;
  /**
   * Someone started saying something: its frames arrive on the utterance as
   * they're spoken, and it ends when they pause for `utteranceGapMs`.
   */
  utterance: Listener<[Utterance]>;
  /** The connection broke; it joins again after `retryInMs`, keeping the same place. */
  reconnecting: Listener<[{ error: FuwaError; retryInMs: number }]>;
  /** Back in after reconnecting. */
  rejoined: Listener<[VoiceState | undefined]>;
  /** Out of the channel for good: `error` says why, unless it was `leave()`. */
  closed: Listener<[FuwaError | undefined]>;
  /** A listener threw. */
  error: Listener<[unknown]>;
}

/** Endings that won't change by trying again: taken out, joined elsewhere, no permission, signed out. */
const FINAL = new Set([Code.FailedPrecondition, Code.PermissionDenied, Code.NotFound, Code.Unauthenticated, Code.InvalidArgument]);

/** Frames sent per SpeakVoice call (100 ms), and how many may wait on the instance (200 ms). */
const BATCH = 5;
const AHEAD = 10;
const FRAME_MS = 20;
/** Waits between tries to rejoin, and how long a stream must stay up before they start over. */
const REJOIN_MIN_MS = 250;
const REJOIN_MAX_MS = 8000;
const STEADY_MS = 10_000;
/** Frames a speech source may run ahead of what's been sent (a second). */
const PULL_AHEAD = 50;

export interface SpeakOptions {
  /** Stops speaking when it aborts, rejecting with CanceledError. */
  signal?: AbortSignal;
  /**
   * Stops when someone starts talking over it (barge-in), resolving with
   * `interrupted`: true for anyone, or a test of who may cut in. The
   * instance drops what it still has queued, so the sound stops at once
   * (older instances play that out, a fraction of a second).
   */
  interruptible?: boolean | ((userId: string) => boolean);
}

export interface SpeakResult {
  /** It stopped early: someone talked over it, or `stopSpeaking()` was called. */
  interrupted: boolean;
  /** Who talked over it. */
  by?: string;
  /** How much sound went out, in milliseconds. */
  sentMs: number;
}

/** Sound to play: Ogg Opus bytes, or a stream of them (a fetch body, a speech service's reply). */
export type OggSource =
  | Uint8Array
  | ArrayBuffer
  | Blob
  | ReadableStream<Uint8Array>
  | AsyncIterable<Uint8Array>
  | Iterable<Uint8Array>
  | { body: ReadableStream<Uint8Array> | null };

interface Speech {
  interruptible: SpeakOptions["interruptible"];
  playing: boolean;
  stopped: boolean;
  by?: string;
  wake: () => void;
}

/**
 * What one person said, from when they start talking until they pause:
 * the unit an agent answers. Its frames come as they're spoken, so a
 * speech-to-text service can start before they've finished. Read it as many
 * times as you like; it keeps its frames (at most `maxUtteranceMs`).
 */
export class Utterance implements AsyncIterable<VoiceFrame> {
  /** Who's talking: their account id. */
  readonly userId: string;
  readonly startedAt: Date;
  #frames: VoiceFrame[] = [];
  #done = false;
  #waiters: (() => void)[] = [];
  #ended: Promise<void>;
  #end!: () => void;

  /** @internal */
  constructor(userId: string) {
    this.userId = userId;
    this.startedAt = new Date();
    this.#ended = new Promise((resolve) => (this.#end = resolve));
  }

  /** Resolves when they pause, leave, or the agent leaves. */
  get ended(): Promise<void> {
    return this.#ended;
  }

  /** Whether it has ended. */
  get done(): boolean {
    return this.#done;
  }

  /** How much has been heard so far, in milliseconds. */
  get durationMs(): number {
    return this.#frames.length * FRAME_MS;
  }

  /** @internal */
  add(frame: VoiceFrame): void {
    if (this.#done) return;
    this.#frames.push(frame);
    this.#wakeAll();
  }

  /** @internal */
  finish(): void {
    if (this.#done) return;
    this.#done = true;
    this.#wakeAll();
    this.#end();
  }

  #wakeAll(): void {
    const waiters = this.#waiters;
    this.#waiters = [];
    for (const wake of waiters) wake();
  }

  /** Its frames from the start, then each as it's spoken, until it ends. */
  async *[Symbol.asyncIterator](): AsyncGenerator<VoiceFrame> {
    for (let i = 0; ; ) {
      if (i < this.#frames.length) {
        yield this.#frames[i++]!;
        continue;
      }
      if (this.#done) return;
      await new Promise<void>((resolve) => this.#waiters.push(resolve));
    }
  }

  /** Its Opus packets as they come. */
  async *opus(): AsyncGenerator<Uint8Array> {
    for await (const frame of this) yield frame.opus;
  }

  /**
   * It as an Ogg Opus file, sent while it's spoken: bytes come every
   * `pageMs` (default 100 ms), and the last when it ends. Speech services
   * that take a stream of Ogg Opus can start on it straight away.
   */
  async *ogg(options: { pageMs?: number } = {}): AsyncGenerator<Uint8Array> {
    const every = Math.max(1, Math.round((options.pageMs ?? 100) / FRAME_MS));
    const writer = new OggOpusWriter();
    let n = 0;
    for await (const frame of this) {
      writer.add(frame.opus);
      if (++n % every === 0) {
        writer.flush();
        yield writer.take();
      }
    }
    yield writer.finish();
  }

  /** The whole utterance as an Ogg Opus file, once it ends. */
  async toOgg(): Promise<Uint8Array<ArrayBuffer>> {
    await this.#ended;
    const writer = new OggOpusWriter();
    for (const frame of this.#frames) writer.add(frame.opus);
    return writer.finish();
  }

  /**
   * It as PCM, through your Opus decoder, as it's spoken. Sound that was
   * lost or not sent (silence) comes as zeros, so the timing stays true.
   */
  async *pcm(decoder: OpusDecoder): AsyncGenerator<Int16Array> {
    let last: number | undefined;
    let size = 0;
    for await (const frame of this) {
      if (last !== undefined && size > 0) {
        const missing = Math.round(((frame.timestamp - last) >>> 0) / 960) - 1;
        if (missing > 0 && missing <= 50) yield new Int16Array(size * missing);
      }
      last = frame.timestamp;
      if (frame.opus.length === 0) {
        if (size > 0) yield new Int16Array(size);
        continue;
      }
      const pcm = decoder.decode(frame.opus);
      size = pcm.length;
      yield pcm;
    }
  }
}

/**
 * Being in a voice channel, without WebRTC: CallService.ListenVoice keeps
 * the place and brings everyone's sound, labelled with whose it is, and
 * SpeakVoice says things. Everyone sees the account in the channel, with its
 * own permissions (CONNECT to join, SPEAK to be heard). Nothing but the
 * instance is ever contacted: no ICE, STUN or TURN.
 *
 *     const voice = await joinVoice(fuwa, { serverId, channelId });
 *     voice.on("speaking", (userId) => console.log(userId, "is talking"));
 *     for await (const frame of voice) { ... }   // or voice.on("frame", ...)
 *     await voice.play(oggOpusBytes);
 *     await voice.leave();
 *
 * Voice channels only: calls in direct messages are end-to-end encrypted
 * between people's apps, and agents don't use direct messages.
 */
export class VoiceConnection implements AsyncIterable<VoiceFrame> {
  readonly serverId: string;
  readonly channelId: string;
  #fuwa: Fuwa;
  #opts: JoinVoiceOptions;
  #session = "";
  #state: VoiceState | undefined;
  #stop = new AbortController();
  #listeners = new Map<string, Set<Listener<never[]>>>();
  #lastHeard = new Map<string, number>();
  #sweeper: ReturnType<typeof setInterval> | undefined;
  #queue: VoiceFrame[] = [];
  #wake: (() => void) | undefined;
  #closed: Promise<void>;
  #done = false;
  #ending: FuwaError | undefined;
  #rejoined: Promise<void> = Promise.resolve();
  #reconnecting = false;
  #unlink: (() => void) | undefined;
  #open = new Map<string, { utterance: Utterance; lastSound: number }>();
  #utteranceReaders = new Set<{ queue: Utterance[]; wake?: () => void }>();
  #speeches = new Set<Speech>();
  #turn: Promise<unknown> = Promise.resolve();
  /** The stream being read now, which a silent stream's watchdog aborts. */
  #current: AbortController | undefined;
  #lastMessage = Date.now();
  #keepalives = false;
  #wentQuiet = false;

  private constructor(fuwa: Fuwa, options: JoinVoiceOptions, first: AsyncIterator<ListenVoiceResponse>) {
    this.#fuwa = fuwa;
    this.#opts = options;
    this.serverId = options.serverId;
    this.channelId = options.channelId;
    options.signal?.addEventListener("abort", () => void this.leave(), { once: true });
    const silenceMs = options.silenceMs ?? 300;
    const gapMs = options.utteranceGapMs ?? 600;
    const quietMs = options.keepaliveTimeoutMs ?? 45_000;
    this.#sweeper = setInterval(() => {
      const now = Date.now();
      if (this.#keepalives && !this.#wentQuiet && !this.#reconnecting && now - this.#lastMessage >= quietMs) {
        // Nothing, not even a keepalive: the stream is gone without saying so.
        this.#wentQuiet = true;
        this.#current?.abort();
      }
      for (const [userId, at] of this.#lastHeard) {
        if (now - at >= silenceMs) {
          this.#lastHeard.delete(userId);
          this.#emit("silent", userId);
        }
      }
      for (const [userId, open] of this.#open) {
        if (now - open.lastSound >= gapMs) {
          this.#open.delete(userId);
          open.utterance.finish();
        }
      }
    }, Math.min(100, silenceMs, gapMs, quietMs));
    this.#closed = this.#run(first);
    this.#closed.catch(() => {});
  }

  /** Joins the voice channel; resolves once the instance says the account is in. */
  static async join(fuwa: Fuwa, options: JoinVoiceOptions): Promise<VoiceConnection> {
    const holder = { session: "", state: undefined as VoiceState | undefined };
    const stop = new AbortController();
    const first = new AbortController();
    const link = () => first.abort();
    stop.signal.addEventListener("abort", link, { once: true });
    const stream = await VoiceConnection.#listen(fuwa, options, "", holder, first);
    const voice = new VoiceConnection(fuwa, options, stream.iterator);
    voice.#session = holder.session;
    voice.#state = holder.state;
    voice.#stop = stop;
    voice.#current = first;
    voice.#unlink = () => stop.signal.removeEventListener("abort", link);
    return voice;
  }

  static async #listen(
    fuwa: Fuwa,
    options: JoinVoiceOptions,
    sessionId: string,
    out: { session: string; state: VoiceState | undefined },
    controller: AbortController,
  ): Promise<{ iterator: AsyncIterator<ListenVoiceResponse>; controller: AbortController }> {
    const iterator = fuwa.calls
      .listenVoice(
        {
          serverId: options.serverId,
          channelId: options.channelId,
          selfMute: options.selfMute ?? false,
          selfDeaf: options.selfDeaf ?? false,
          sessionId,
        },
        { signal: controller.signal },
      )
      [Symbol.asyncIterator]();
    try {
      const first = await iterator.next();
      if (first.done || first.value.event.case !== "joined") {
        throw new UnavailableError(Code.Unavailable, "the instance didn't say the agent is in the channel");
      }
      out.session = first.value.event.value.sessionId;
      out.state = first.value.event.value.state;
      return { iterator, controller };
    } catch (err) {
      controller.abort();
      throw toFuwaError(err, "fuwa.v1.CallService/ListenVoice");
    }
  }

  /** The place in the call: SpeakVoice and rejoining name it. */
  get sessionId(): string {
    return this.#session;
  }

  /** How the account is in the channel, as the instance said when it joined. */
  get state(): VoiceState | undefined {
    return this.#state;
  }

  /** Who's sending sound right now (account ids). */
  get speaking(): ReadonlySet<string> {
    return new Set(this.#lastHeard.keys());
  }

  /** Resolves when out of the channel: after `leave()`, or (rejecting) when taken out. */
  get closed(): Promise<void> {
    return this.#closed;
  }

  on<K extends keyof VoiceEvents>(name: K, listener: VoiceEvents[K]): () => void {
    let set = this.#listeners.get(name);
    if (!set) this.#listeners.set(name, (set = new Set()));
    set.add(listener as Listener<never[]>);
    return () => set.delete(listener as Listener<never[]>);
  }

  /**
   * Every frame heard, while in the channel. Only one loop should read it; a
   * reader that falls more than ten seconds behind loses the oldest frames.
   */
  async *[Symbol.asyncIterator](): AsyncGenerator<VoiceFrame> {
    for (;;) {
      const next = this.#queue.shift();
      if (next) {
        yield next;
        continue;
      }
      if (this.#done) {
        if (this.#ending) throw this.#ending;
        return;
      }
      await new Promise<void>((resolve) => (this.#wake = resolve));
    }
  }

  /**
   * Each utterance as someone starts it (see the `utterance` event). Only
   * utterances begun after the loop starts come; a loop more than 100
   * behind loses the oldest.
   */
  async *utterances(): AsyncGenerator<Utterance> {
    const reader: { queue: Utterance[]; wake?: () => void } = { queue: [] };
    this.#utteranceReaders.add(reader);
    try {
      for (;;) {
        const next = reader.queue.shift();
        if (next) {
          yield next;
          continue;
        }
        if (this.#done) {
          if (this.#ending) throw this.#ending;
          return;
        }
        await new Promise<void>((resolve) => (reader.wake = resolve));
      }
    } finally {
      this.#utteranceReaders.delete(reader);
    }
  }

  /** Whether the account is saying something now (or has things waiting to be said). */
  get talking(): boolean {
    return this.#speeches.size > 0;
  }

  /**
   * Says Opus frames (48 kHz, 20 ms each, mono or stereo), from a list or as
   * an encoder or speech service makes them: each goes out as soon as it
   * comes, and a source faster than real time is paced to about how fast it
   * plays. Resolves about when the last one is heard. Things said one after
   * another wait their turn. Needs SPEAK and not being server muted.
   */
  speak(frames: Iterable<Uint8Array> | AsyncIterable<Uint8Array>, options: SpeakOptions = {}): Promise<SpeakResult> {
    const speech: Speech = { interruptible: options.interruptible, playing: false, stopped: false, wake: () => {} };
    this.#speeches.add(speech);
    const run = this.#turn.then(() => this.#speakNow(frames, speech, options.signal));
    this.#turn = run.catch(() => {});
    return run.finally(() => this.#speeches.delete(speech));
  }

  /**
   * Plays Ogg Opus (.ogg or .opus, as ffmpeg makes with `-c:a libopus
   * -frame_duration 20`): a whole file, or a stream such as a speech
   * service's reply, which starts playing as soon as its first page comes.
   * Its sound must be in 20 ms frames (packets of two or three are split).
   */
  async play(source: OggSource, options: SpeakOptions = {}): Promise<SpeakResult> {
    if (source instanceof Uint8Array || source instanceof ArrayBuffer || (typeof Blob !== "undefined" && source instanceof Blob)) {
      const bytes =
        source instanceof Uint8Array ? source : new Uint8Array(source instanceof Blob ? await source.arrayBuffer() : source);
      const frames = readOggOpus(bytes).packets.flatMap((p) => twentyMs(p));
      return this.speak(frames, options);
    }
    const stream =
      "body" in source && !(Symbol.asyncIterator in source) && !("getReader" in source)
        ? source.body
        : (source as ReadableStream<Uint8Array> | AsyncIterable<Uint8Array> | Iterable<Uint8Array>);
    if (!stream) throw new TypeError("there's no sound to play: the response has no body");
    try {
      return await this.speak(
        (async function* () {
          for await (const packet of oggOpusPackets(chunks(stream))) yield* twentyMs(packet);
        })(),
        options,
      );
    } finally {
      // Stopped before its turn, or part way: let the download go. (After
      // reading it all, this does nothing.)
      if ("getReader" in stream) {
        if (!stream.locked) void stream.cancel().catch(() => {});
      } else if (Symbol.asyncIterator in stream) {
        void Promise.resolve(stream[Symbol.asyncIterator]().return?.()).catch(() => {});
      }
    }
  }

  /**
   * Says raw sound (16-bit PCM, interleaved when stereo), as a speech
   * service makes it, through your Opus encoder: see OpusEncoder.
   */
  speakPcm(
    pcm: Iterable<Int16Array> | AsyncIterable<Int16Array>,
    options: SpeakOptions & PcmFormat & { encoder: OpusEncoder },
  ): Promise<SpeakResult> {
    const { encoder } = options;
    return this.speak(
      (async function* () {
        for await (const frame of pcmFrames(pcm, options)) yield encoder.encode(frame);
      })(),
      options,
    );
  }

  /** Stops saying anything: what's being said now and everything waiting its turn. */
  stopSpeaking(): void {
    for (const speech of this.#speeches) this.#interrupt(speech);
  }

  /** Changes the account's own mute and deafen, as people do. */
  async setState(state: { selfMute?: boolean; selfDeaf?: boolean }): Promise<VoiceState | undefined> {
    this.#opts = { ...this.#opts, ...state };
    const res = await this.#fuwa.calls.keepVoice({
      serverId: this.serverId,
      sessionId: this.#session,
      channelId: this.channelId,
      selfMute: this.#opts.selfMute ?? false,
      selfDeaf: this.#opts.selfDeaf ?? false,
    });
    this.#state = res.state ?? this.#state;
    return this.#state;
  }

  /** Leaves the channel. */
  async leave(): Promise<void> {
    if (!this.#done) {
      this.#stop.abort();
      await this.#fuwa.calls.leaveVoice({ serverId: this.serverId, sessionId: this.#session }).catch(() => {});
    }
    await this.#closed.catch(() => {});
  }

  async #say(frames: Uint8Array[], signal?: AbortSignal): Promise<number> {
    if (this.#done) {
      throw this.#ending ?? new FailedPreconditionError(Code.FailedPrecondition, "not in the voice channel any more");
    }
    const send = () =>
      this.#fuwa.calls.speakVoice({ serverId: this.serverId, sessionId: this.#session, frames }, { signal });
    let res;
    try {
      res = await send();
    } catch (err) {
      // Rejoining after a restart: wait for it, then once more.
      if (!(err instanceof UnavailableError) && !this.#reconnecting) throw err;
      await Promise.race([this.#rejoined, sleep(5000, signal)]);
      res = await send();
    }
    return res.queued;
  }

  async #speakNow(
    source: Iterable<Uint8Array> | AsyncIterable<Uint8Array>,
    speech: Speech,
    signal?: AbortSignal,
  ): Promise<SpeakResult> {
    if (signal?.aborted) throw new CanceledError(Code.Canceled, "stopped speaking");
    if (speech.stopped) return { interrupted: true, by: speech.by, sentMs: 0 };
    const iterator =
      Symbol.asyncIterator in source
        ? (source as AsyncIterable<Uint8Array>)[Symbol.asyncIterator]()
        : (source as Iterable<Uint8Array>)[Symbol.iterator]();
    const pending: Uint8Array[] = [];
    let ended = false;
    let failed = false;
    let failure: unknown;
    // One wake-up for everything this waits on: a frame, room, stopping.
    let wake: (() => void) | undefined;
    speech.wake = () => {
      const w = wake;
      wake = undefined;
      w?.();
    };
    const nap = (ms: number) =>
      new Promise<void>((resolve) => {
        const timer = setTimeout(() => speech.wake(), Math.max(0, ms));
        wake = () => {
          clearTimeout(timer);
          resolve();
        };
      });
    const onAbort = () => speech.wake();
    signal?.addEventListener("abort", onAbort);
    speech.playing = true;
    let room: (() => void) | undefined;
    void (async () => {
      try {
        for (;;) {
          if (speech.stopped || signal?.aborted) return;
          if (pending.length >= PULL_AHEAD) {
            await new Promise<void>((resolve) => (room = resolve));
            continue;
          }
          const { done, value } = await iterator.next();
          if (done) return;
          pending.push(value);
          speech.wake();
        }
      } catch (err) {
        failed = true;
        failure = err;
      } finally {
        ended = true;
        speech.wake();
      }
    })();
    let queued = 0;
    let at = Date.now();
    let sent = 0;
    const ahead = () => Math.max(0, queued - (Date.now() - at) / FRAME_MS);
    try {
      for (;;) {
        if (signal?.aborted) throw new CanceledError(Code.Canceled, "stopped speaking");
        if (speech.stopped) break;
        if (failed) throw failure;
        if (this.#done) {
          throw this.#ending ?? new FailedPreconditionError(Code.FailedPrecondition, "not in the voice channel any more");
        }
        if (ended && pending.length === 0) {
          // Let what's queued play out.
          const left = ahead();
          if (left <= 0) break;
          await nap(left * FRAME_MS);
          continue;
        }
        const left = ahead();
        if (left > AHEAD) {
          await nap((left - AHEAD) * FRAME_MS);
          continue;
        }
        // Send a batch, or whatever there is when the instance is about to run dry.
        if (pending.length >= BATCH || (pending.length > 0 && (left <= 1 || ended))) {
          const batch = pending.splice(0, BATCH);
          room?.();
          room = undefined;
          queued = await this.#say(batch, signal);
          at = Date.now();
          sent += batch.length;
          continue;
        }
        await nap(pending.length > 0 ? (left - 1) * FRAME_MS : 60_000);
      }
    } finally {
      speech.playing = false;
      signal?.removeEventListener("abort", onAbort);
      if (!ended) {
        room?.();
        void Promise.resolve(iterator.return?.()).catch(() => {});
      }
      // Cut short: drop what the instance still has queued, so it stops now
      // rather than a fraction of a second later (older instances ignore it).
      if (sent > 0 && (speech.stopped || signal?.aborted) && !this.#done) {
        await this.#fuwa.calls
          .speakVoice(
            { serverId: this.serverId, sessionId: this.#session, frames: [], interrupt: true },
            // An instance that never answers mustn't hold up what's said next.
            { timeoutMs: 2000 },
          )
          .catch(() => {});
      }
    }
    return { interrupted: speech.stopped, by: speech.by, sentMs: sent * FRAME_MS };
  }

  #interrupt(speech: Speech, by?: string): void {
    if (speech.stopped) return;
    speech.stopped = true;
    speech.by = by;
    speech.wake();
  }

  async #run(first: AsyncIterator<ListenVoiceResponse>): Promise<void> {
    let iterator: AsyncIterator<ListenVoiceResponse> | undefined = first;
    let wait = REJOIN_MIN_MS;
    let upSince = Date.now();
    try {
      for (;;) {
        let error: FuwaError;
        try {
          for (;;) {
            const { done, value } = await iterator!.next();
            if (done) break;
            this.#lastMessage = Date.now();
            if (value.event.case === "frame") this.#heard(value.event.value);
            else if (value.event.case === "keepalive") this.#keepalives = true;
          }
          error = new UnavailableError(Code.Unavailable, "the instance closed the voice stream");
        } catch (cause) {
          error = toFuwaError(cause, "fuwa.v1.CallService/ListenVoice");
        }
        if (this.#stop.signal.aborted) return;
        if (this.#wentQuiet) {
          this.#wentQuiet = false;
          error = new UnavailableError(Code.Unavailable, "the voice stream went quiet");
        }
        if (FINAL.has(error.code)) throw error;
        // Only a stream that stayed up a while starts the waits over, so one
        // that keeps dropping right after it joins backs off.
        if (Date.now() - upSince >= STEADY_MS) wait = REJOIN_MIN_MS;
        // A deploy or a dropped connection: join again, keeping the place.
        let rejoined!: () => void;
        this.#rejoined = new Promise((resolve) => (rejoined = resolve));
        this.#reconnecting = true;
        for (;;) {
          const retryInMs = Math.round(wait / 2 + (Math.random() * wait) / 2);
          this.#emit("reconnecting", { error, retryInMs });
          try {
            await sleep(retryInMs, this.#stop.signal);
          } catch {
            return;
          }
          wait = Math.min(wait * 2, REJOIN_MAX_MS);
          // Each try has its own controller, which leaving aborts: a failed try
          // aborts only itself, never the connection.
          const attempt = new AbortController();
          const stop = () => attempt.abort();
          this.#unlink?.();
          this.#stop.signal.addEventListener("abort", stop, { once: true });
          this.#unlink = () => this.#stop.signal.removeEventListener("abort", stop);
          try {
            const out = { session: this.#session, state: this.#state };
            const stream = await VoiceConnection.#listen(this.#fuwa, this.#opts, this.#session, out, attempt);
            upSince = Date.now();
            this.#current = attempt;
            this.#lastMessage = upSince;
            this.#session = out.session;
            this.#state = out.state;
            iterator = stream.iterator;
            this.#reconnecting = false;
            rejoined();
            this.#emit("rejoined", this.#state);
            break;
          } catch (err) {
            this.#stop.signal.removeEventListener("abort", stop);
            if (this.#stop.signal.aborted) return;
            error = toFuwaError(err);
            if (FINAL.has(error.code)) throw error;
          }
        }
      }
    } catch (err) {
      this.#ending = toFuwaError(err);
      throw this.#ending;
    } finally {
      this.#done = true;
      clearInterval(this.#sweeper);
      for (const userId of this.#lastHeard.keys()) this.#emit("silent", userId);
      this.#lastHeard.clear();
      for (const open of this.#open.values()) open.utterance.finish();
      this.#open.clear();
      for (const speech of this.#speeches) speech.wake();
      for (const reader of this.#utteranceReaders) reader.wake?.();
      this.#wake?.();
      this.#emit("closed", this.#ending);
    }
  }

  #heard(f: { userId: string; opus: Uint8Array; timestamp: number }): void {
    const frame: VoiceFrame = { userId: f.userId, opus: f.opus, timestamp: f.timestamp };
    // Opus sends a packet of a byte or two for silence (DTX): that's not speaking.
    const loud = frame.opus.length > 2;
    const now = Date.now();
    if (loud) {
      if (!this.#lastHeard.has(frame.userId)) {
        this.#emit("speaking", frame.userId);
        for (const speech of this.#speeches) {
          const may = speech.interruptible;
          if (!speech.playing || !may) continue;
          try {
            if (may === true || may(frame.userId)) this.#interrupt(speech, frame.userId);
          } catch (err) {
            this.#fail("interruptible", err);
          }
        }
      }
      this.#lastHeard.set(frame.userId, now);
    }
    let open = this.#open.get(frame.userId);
    if (open && open.utterance.durationMs >= (this.#opts.maxUtteranceMs ?? 60_000)) {
      this.#open.delete(frame.userId);
      open.utterance.finish();
      open = undefined;
    }
    if (!open && loud && (this.#utteranceReaders.size > 0 || this.#listeners.get("utterance")?.size)) {
      open = { utterance: new Utterance(frame.userId), lastSound: now };
      this.#open.set(frame.userId, open);
      for (const reader of this.#utteranceReaders) {
        reader.queue.push(open.utterance);
        if (reader.queue.length > 100) reader.queue.shift(); // a reader far behind loses the oldest
        reader.wake?.();
        reader.wake = undefined;
      }
      this.#emit("utterance", open.utterance);
    }
    if (open) {
      open.utterance.add(frame);
      if (loud) open.lastSound = now;
    }
    this.#emit("frame", frame);
    this.#queue.push(frame);
    if (this.#queue.length > 500) this.#queue.shift(); // ten seconds behind: drop the oldest
    this.#wake?.();
    this.#wake = undefined;
  }

  #emit(name: string, ...args: unknown[]): void {
    const set = this.#listeners.get(name);
    if (!set) return;
    for (const listener of set) {
      try {
        const out = (listener as Listener<unknown[]>)(...args);
        if (out instanceof Promise) out.catch((err) => this.#fail(name, err));
      } catch (err) {
        this.#fail(name, err);
      }
    }
  }

  #fail(name: string, err: unknown): void {
    if (name === "error") return;
    if (this.#listeners.get("error")?.size) this.#emit("error", err);
    else {
      const e = err instanceof Error ? err : new Error(String(err));
      console.error(`[fuwa voice] ${e.name}: ${e.message}`);
    }
  }
}

/** A packet as the 20 ms frames a voice channel takes, or a TypeError saying why it can't be. */
function twentyMs(packet: Uint8Array): Uint8Array[] {
  const frames = splitOpusPacket(packet);
  const wrong = frames.find((f) => opusPacketDuration(f) !== FRAME_MS);
  if (wrong) {
    throw new TypeError(
      `voice channels take 20 ms Opus frames, and this sound has ${opusPacketDuration(wrong)} ms ones (re-encode with -frame_duration 20)`,
    );
  }
  return frames;
}

/** Joins a voice channel (see VoiceConnection). */
export function joinVoice(fuwa: Fuwa, options: JoinVoiceOptions): Promise<VoiceConnection> {
  return VoiceConnection.join(fuwa, options);
}
