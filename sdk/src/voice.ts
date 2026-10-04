import type { Fuwa } from "./client.js";
import { CanceledError, Code, FailedPreconditionError, FuwaError, UnavailableError, toFuwaError } from "./errors.js";
import type { ListenVoiceResponse } from "./gen/fuwa/v1/call_pb.js";
import type { VoiceState } from "./gen/fuwa/v1/types_pb.js";
import { opusPacketDuration, readOggOpus } from "./ogg.js";
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

  private constructor(fuwa: Fuwa, options: JoinVoiceOptions, first: AsyncIterator<ListenVoiceResponse>) {
    this.#fuwa = fuwa;
    this.#opts = options;
    this.serverId = options.serverId;
    this.channelId = options.channelId;
    options.signal?.addEventListener("abort", () => void this.leave(), { once: true });
    const silenceMs = options.silenceMs ?? 300;
    this.#sweeper = setInterval(() => {
      const now = Date.now();
      for (const [userId, at] of this.#lastHeard) {
        if (now - at >= silenceMs) {
          this.#lastHeard.delete(userId);
          this.#emit("silent", userId);
        }
      }
    }, Math.min(100, silenceMs));
    this.#closed = this.#run(first);
    this.#closed.catch(() => {});
  }

  /** Joins the voice channel; resolves once the instance says the account is in. */
  static async join(fuwa: Fuwa, options: JoinVoiceOptions): Promise<VoiceConnection> {
    const holder = { session: "", state: undefined as VoiceState | undefined };
    const stream = await VoiceConnection.#listen(fuwa, options, "", holder, new AbortController());
    const voice = new VoiceConnection(fuwa, options, stream.iterator);
    voice.#session = holder.session;
    voice.#state = holder.state;
    voice.#stop = stream.controller;
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
   * Says Opus frames (48 kHz, 20 ms each, mono or stereo), from a list or as
   * an encoder makes them, sending them about as fast as they play. Resolves
   * about when the last one is heard. Needs SPEAK and not being server muted.
   */
  async speak(frames: Iterable<Uint8Array> | AsyncIterable<Uint8Array>, options: { signal?: AbortSignal } = {}): Promise<void> {
    let batch: Uint8Array[] = [];
    let queued = 0;
    for await (const frame of frames) {
      if (options.signal?.aborted) throw new CanceledError(Code.Canceled, "stopped speaking");
      batch.push(frame);
      if (batch.length === BATCH) {
        queued = await this.#say(batch, options.signal);
        batch = [];
      }
    }
    if (batch.length) queued = await this.#say(batch, options.signal);
    if (queued > 0) await sleep(queued * FRAME_MS, options.signal);
  }

  /**
   * Plays an Ogg Opus file (.ogg or .opus, as ffmpeg makes with
   * `-c:a libopus -frame_duration 20`). Its packets must be 20 ms each.
   */
  async play(file: Uint8Array | ArrayBuffer | Blob, options: { signal?: AbortSignal } = {}): Promise<void> {
    const bytes =
      file instanceof Uint8Array ? file : new Uint8Array(file instanceof Blob ? await file.arrayBuffer() : file);
    const { packets } = readOggOpus(bytes);
    const wrong = packets.find((p) => opusPacketDuration(p) !== FRAME_MS);
    if (wrong) {
      throw new TypeError(
        `voice channels take 20 ms Opus frames, and this file has ${opusPacketDuration(wrong)} ms ones (re-encode with -frame_duration 20)`,
      );
    }
    await this.speak(packets, options);
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
    if (res.queued > AHEAD) await sleep((res.queued - AHEAD) * FRAME_MS, signal);
    return Math.min(res.queued, AHEAD);
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
            if (value.event.case === "frame") this.#heard(value.event.value);
          }
          error = new UnavailableError(Code.Unavailable, "the instance closed the voice stream");
        } catch (cause) {
          error = toFuwaError(cause, "fuwa.v1.CallService/ListenVoice");
        }
        if (this.#stop.signal.aborted) return;
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
      this.#wake?.();
      this.#emit("closed", this.#ending);
    }
  }

  #heard(f: { userId: string; opus: Uint8Array; timestamp: number }): void {
    const frame: VoiceFrame = { userId: f.userId, opus: f.opus, timestamp: f.timestamp };
    // Opus sends a packet of a byte or two for silence (DTX): that's not speaking.
    if (frame.opus.length > 2) {
      if (!this.#lastHeard.has(frame.userId)) this.#emit("speaking", frame.userId);
      this.#lastHeard.set(frame.userId, Date.now());
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

/** Joins a voice channel (see VoiceConnection). */
export function joinVoice(fuwa: Fuwa, options: JoinVoiceOptions): Promise<VoiceConnection> {
  return VoiceConnection.join(fuwa, options);
}
