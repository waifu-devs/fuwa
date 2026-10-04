import type { Fuwa } from "./client.js";
import { CanceledError, Code, FuwaError, UnavailableError, toFuwaError } from "./errors.js";
import type { ServerHead } from "./gen/fuwa/v1/event_pb.js";
import type { Event } from "./gen/fuwa/v1/types_pb.js";
import { DEFAULT_RETRY, backoff, sleep } from "./retry.js";

/** Every kind of event, by its payload's name, such as "messageCreated". */
export type EventKind = NonNullable<Event["payload"]["case"]>;

/** The payload of one kind of event, such as MessageCreated for "messageCreated". */
export type EventPayload<K extends EventKind> = Extract<Event["payload"], { case: K }>["value"];

/** What a follower yields. */
export type FollowUpdate =
  /** An event, live or replayed. */
  | { type: "event"; event: Event }
  /** Any replay is done and the stream is live; where each server stands. */
  | { type: "ready"; heads: ServerHead[] }
  /** With `followNewServers`: a server the caller joined or was added to, followed from here on. */
  | { type: "followed"; head: ServerHead }
  /** The stream broke; it reconnects after `retryInMs`, resuming from the last event seen. */
  | { type: "disconnected"; error: FuwaError; retryInMs: number };

export interface FollowOptions {
  /** The servers to follow. Change them later with `follower.setServers`. */
  servers: Iterable<string>;
  /**
   * Where to resume each server from: the last sequence the app handled.
   * Servers without one start with new events only. Keep `follower.cursors`
   * somewhere to resume after a restart without missing anything.
   */
  cursors?: Record<string, bigint> | Map<string, bigint>;
  /**
   * Also follow servers the caller joins or is added to while the stream is
   * open, announced as "followed". Instances with the `agent-streams`
   * feature do this; older ones refuse a stream with no servers and ignore
   * the request otherwise.
   */
  followNewServers?: boolean;
  /** Stops following. */
  signal?: AbortSignal;
  /**
   * The instance sends a heartbeat every 25 seconds; this long with nothing
   * means the connection is gone. Default 70 s.
   */
  silenceMs?: number;
  /** Waits between reconnects. Default from 400 ms up to 20 s, with jitter. */
  baseDelayMs?: number;
  maxDelayMs?: number;
}

const STREAM_ENDED = "the instance closed the event stream";
const SILENT = "the event stream went quiet";

/**
 * Follows servers' events over one EventService.Subscribe stream, for as long
 * as it's iterated: it reconnects with backoff when the stream breaks and
 * resumes from each server's last sequence, so nothing is missed or seen
 * twice. A signed-out or forbidden answer ends it with that error.
 *
 *     const follower = new EventFollower(fuwa, { servers: ["srv_1"] });
 *     for await (const update of follower) {
 *       if (update.type === "event") console.log(update.event.payload.case);
 *     }
 */
export class EventFollower implements AsyncIterable<FollowUpdate> {
  /** The last sequence seen per server. */
  readonly cursors: Map<string, bigint>;
  #servers: Set<string>;
  #fuwa: Fuwa;
  #opts: FollowOptions;
  #restart: AbortController | null = null;
  #wake: (() => void) | null = null;
  #iterating = false;

  constructor(fuwa: Fuwa, options: FollowOptions) {
    this.#fuwa = fuwa;
    this.#opts = options;
    this.#servers = new Set(options.servers);
    this.cursors = new Map(
      options.cursors instanceof Map ? options.cursors : Object.entries(options.cursors ?? {}),
    );
  }

  /** The servers being followed. */
  get servers(): ReadonlySet<string> {
    return this.#servers;
  }

  /** Follows these servers instead, reconnecting at once if the list changed. */
  setServers(servers: Iterable<string>): void {
    const next = new Set(servers);
    if (next.size === this.#servers.size && [...next].every((id) => this.#servers.has(id))) return;
    this.#servers = next;
    for (const id of this.cursors.keys()) if (!next.has(id)) this.cursors.delete(id);
    this.#restart?.abort();
    this.#wake?.();
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<FollowUpdate> {
    if (this.#iterating) throw new Error("an EventFollower can only be iterated once at a time");
    this.#iterating = true;
    const outer = this.#opts.signal;
    const silenceMs = this.#opts.silenceMs ?? 70_000;
    const delays = {
      ...DEFAULT_RETRY,
      baseDelayMs: this.#opts.baseDelayMs ?? DEFAULT_RETRY.baseDelayMs,
      maxDelayMs: this.#opts.maxDelayMs ?? DEFAULT_RETRY.maxDelayMs,
    };
    let failures = 0;
    try {
      while (!outer?.aborted) {
        if (this.#servers.size === 0 && !this.#opts.followNewServers) {
          // Nothing to follow: wait for setServers or the end.
          await new Promise<void>((resolve) => {
            this.#wake = resolve;
            outer?.addEventListener("abort", () => resolve(), { once: true });
          });
          this.#wake = null;
          continue;
        }
        const restart = new AbortController();
        this.#restart = restart;
        const stop = () => restart.abort();
        outer?.addEventListener("abort", stop, { once: true });
        let silence: ReturnType<typeof setTimeout> | undefined;
        let quiet = false;
        const heard = () => {
          clearTimeout(silence);
          silence = setTimeout(() => {
            quiet = true;
            restart.abort();
          }, silenceMs);
        };
        let error: FuwaError;
        try {
          heard();
          const servers = [...this.#servers].map((serverId) => ({
            serverId,
            afterSequence: this.cursors.get(serverId),
          }));
          const followNewServers = this.#opts.followNewServers ?? false;
          for await (const res of this.#fuwa.events.subscribe(
            { servers, followNewServers },
            { signal: restart.signal, timeoutMs: 0 },
          )) {
            heard();
            if (res.event) {
              const e = res.event;
              if (e.sequence > 0n) {
                const last = this.cursors.get(e.serverId);
                // A replay can't repeat, but a server's events are only ever handed out once.
                if (last !== undefined && e.sequence <= last) continue;
                this.cursors.set(e.serverId, e.sequence);
              }
              if (e.payload.case === "serverDeleted" || (e.payload.case === "memberLeft" && e.sequence === 0n)) {
                // Not followed any more, by the instance's say.
                this.#servers.delete(e.serverId);
                this.cursors.delete(e.serverId);
              }
              failures = 0;
              yield { type: "event", event: e };
            } else if (res.ready) {
              for (const head of res.ready.servers) {
                if (!this.cursors.has(head.serverId)) this.cursors.set(head.serverId, head.sequence);
              }
              failures = 0;
              yield { type: "ready", heads: res.ready.servers };
            } else if (res.followed) {
              const head = res.followed;
              // Joined while the stream was open: its events after this come next.
              if (this.#servers.has(head.serverId)) continue;
              this.#servers.add(head.serverId);
              if (!this.cursors.has(head.serverId)) this.cursors.set(head.serverId, head.sequence);
              yield { type: "followed", head };
            }
          }
          error = new UnavailableError(Code.Unavailable, STREAM_ENDED);
        } catch (cause) {
          if (outer?.aborted) return;
          if (restart.signal.aborted && !quiet) continue; // setServers: reconnect at once
          error = quiet ? new UnavailableError(Code.Unavailable, SILENT) : toFuwaError(cause);
          if (error instanceof CanceledError) error = new UnavailableError(Code.Unavailable, SILENT);
        } finally {
          clearTimeout(silence);
          outer?.removeEventListener("abort", stop);
          this.#restart = null;
        }
        if (!error.retryable) throw error;
        const retryInMs = backoff(Math.min(failures, 16), delays);
        failures++;
        yield { type: "disconnected", error, retryInMs };
        try {
          await sleep(retryInMs, outer);
        } catch {
          return;
        }
      }
    } finally {
      this.#iterating = false;
    }
  }
}

/**
 * A server's stored events after `after` (0 for all of them), page by page
 * through EventService.ListEvents. Leaves out events about channels the
 * caller can't see now.
 */
export async function* listEvents(
  fuwa: Fuwa,
  serverId: string,
  options: { after?: bigint; pageSize?: number; signal?: AbortSignal } = {},
): AsyncGenerator<Event> {
  let after = options.after ?? 0n;
  for (;;) {
    const page = await fuwa.events.listEvents(
      { serverId, afterSequence: after, limit: options.pageSize ?? 100 },
      { signal: options.signal },
    );
    for (const e of page.events) {
      after = e.sequence > after ? e.sequence : after;
      yield e;
    }
    if (!page.hasMore || page.events.length === 0) return;
  }
}
