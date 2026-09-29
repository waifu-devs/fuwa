import { Code } from "@connectrpc/connect";
import { Effect, Fiber, FiberSet, Schedule, Stream, SubscriptionRef } from "effect";
import type { SubscribeResponse } from "@/gen/fuwa/v1/event_pb";
import type { Event } from "@/gen/fuwa/v1/types_pb";
import { onLiveEvent } from "@/lib/notify";
import { makeApi, type Api } from "./client";
import { FuwaError, call, toFuwaError } from "./errors";
import { instanceKey, loadSaved, storeSaved, type SavedInstance } from "./saved";
import {
  addServer,
  applyEvent,
  applySnapshot,
  emptyInstance,
  notificationKey,
  patchInstance,
  removeServer,
  store,
  updateInstance,
} from "./store";

/**
 * Keeps every instance in the store in step with its server: loads who you
 * are and which servers you're in, follows all of them over one event
 * stream, and reconnects with backoff, resuming from the last event seen so
 * nothing is missed or applied twice.
 */

/** Retry quickly at first, then every 20 seconds at most. */
const backoff = Schedule.exponential("400 millis", 2).pipe(
  Schedule.union(Schedule.spaced("20 seconds")),
  Schedule.jittered,
);
const retryPolicy = backoff.pipe(Schedule.whileInput((e: FuwaError) => e.retryable));

/** The server sends a heartbeat every 25 seconds; this long without anything means the connection is gone. */
const SILENCE = "70 seconds";

type Engine = {
  url: string;
  api: Api;
  token: string | null;
  /** The servers the stream follows. Changing it resubscribes. */
  followed: SubscriptionRef.SubscriptionRef<readonly string[]>;
  fiber: Fiber.RuntimeFiber<void, never> | null;
};

const engines = new Map<string, Engine>();

export function engine(key: string): Engine {
  const e = engines.get(key);
  if (!e) throw new Error(`unknown instance ${key}`);
  return e;
}

function persist() {
  const list: SavedInstance[] = store.get().order.flatMap((key) => {
    const e = engines.get(key);
    return e ? [{ url: e.url, token: e.token }] : [];
  });
  storeSaved(list);
}

/** Loads the saved instances and starts following them. Call once at startup. */
export function restore() {
  for (const saved of loadSaved()) addInstance(saved.url, saved.token);
}

/** Adds an instance (or updates its token) and (re)starts syncing it. Returns its key. */
export function addInstance(url: string, token: string | null): string {
  const key = instanceKey(url);
  const existing = engines.get(key);
  if (existing?.fiber) Effect.runFork(Fiber.interrupt(existing.fiber));
  const e: Engine = existing ?? {
    url,
    api: makeApi(url, () => engines.get(key)?.token ?? null),
    token,
    followed: Effect.runSync(SubscriptionRef.make<readonly string[]>([])),
    fiber: null,
  };
  e.token = token;
  engines.set(key, e);
  store.update((s) => ({
    ...s,
    instances: { ...s.instances, [key]: emptyInstance(key, url) },
    order: s.order.includes(key) ? s.order : [...s.order, key],
  }));
  persist();
  e.fiber = Effect.runFork(run(key, e));
  return key;
}

/** Forgets an instance: stops syncing and drops its token from this browser. */
export function removeInstance(key: string) {
  const e = engines.get(key);
  if (e?.fiber) Effect.runFork(Fiber.interrupt(e.fiber));
  engines.delete(key);
  store.update((s) => {
    const { [key]: _, ...instances } = s.instances;
    return { ...s, instances, order: s.order.filter((k) => k !== key), focus: s.focus?.instance === key ? null : s.focus };
  });
  persist();
}

/** Starts or stops following a server after joining, creating or leaving it. */
export const follow = (key: string, serverId: string) =>
  SubscriptionRef.update(engine(key).followed, (ids) => (ids.includes(serverId) ? ids : [...ids, serverId]));
export const unfollow = (key: string, serverId: string) =>
  SubscriptionRef.update(engine(key).followed, (ids) => ids.filter((id) => id !== serverId));

const run = (key: string, e: Engine): Effect.Effect<void, never> =>
  Effect.gen(function* () {
    const api = e.api;
    const retrying = <A>(effect: Effect.Effect<A, FuwaError>) =>
      effect.pipe(
        Effect.tapError((err) =>
          Effect.sync(() => err.retryable && patchInstance(key, { connection: "offline", problem: err.message })),
        ),
        Effect.retry(retryPolicy),
      );

    const { node } = yield* retrying(call((signal) => api.node.getNode({}, { signal })));
    patchInstance(key, { node: node ?? null, problem: null });
    if (!e.token) {
      patchInstance(key, { connection: "signed-out" });
      return;
    }

    const me = yield* retrying(call((signal) => api.auth.getMe({}, { signal })));
    patchInstance(key, { me: me.user ?? null, admin: me.admin });
    // Notification settings follow the account; an older instance without them just has none.
    const notifications = yield* call((signal) => api.account.getNotificationSettings({}, { signal })).pipe(
      Effect.map((r) => Object.fromEntries(r.settings.map((n) => [notificationKey(n.serverId, n.channelId), n]))),
      Effect.catchAll((err) => (err.signedOut ? Effect.fail(err) : Effect.succeed({}))),
    );
    patchInstance(key, { notifications });

    const { servers } = yield* retrying(call((signal) => api.servers.listServers({}, { signal })));
    updateInstance(key, (i) => servers.reduce(addServer, i));
    yield* SubscriptionRef.set(
      e.followed,
      servers.map((s) => s.id),
    );
    patchInstance(key, { connection: servers.length ? "connecting" : "live" });

    yield* followEvents(key, api, e.followed);
  }).pipe(
    Effect.scoped,
    Effect.catchAll((err) =>
      Effect.sync(() => {
        if (err.signedOut) {
          // The session expired or was revoked elsewhere.
          e.token = null;
          persist();
          patchInstance(key, { connection: "signed-out", problem: "Your session ended. Sign in again." });
        } else {
          patchInstance(key, { connection: "offline", problem: err.message });
        }
      }),
    ),
  );

const followEvents = (key: string, api: Api, followed: SubscriptionRef.SubscriptionRef<readonly string[]>) =>
  Effect.gen(function* () {
    const snapshots = yield* FiberSet.make<void, never>();
    /** The last event applied (or skipped as already known) per server. */
    const cursors = new Map<string, bigint>();
    /** Events for servers whose snapshot is still loading, applied once it lands. */
    const held = new Map<string, Event[]>();

    const subscribe = (ids: readonly string[]) =>
      Stream.suspend(() => {
        const controller = new AbortController();
        const request = { servers: ids.map((serverId) => ({ serverId, afterSequence: cursors.get(serverId) })) };
        return Stream.fromAsyncIterable<SubscribeResponse, FuwaError>(
          api.events.subscribe(request, { signal: controller.signal }),
          toFuwaError,
        ).pipe(Stream.ensuring(Effect.sync(() => controller.abort())));
      }).pipe(
        Stream.timeoutFail(() => new FuwaError({ code: Code.Unavailable, message: "lost the connection" }), SILENCE),
        Stream.tapError((err) =>
          Effect.sync(() => err.retryable && patchInstance(key, { connection: "reconnecting", problem: err.message })),
        ),
        Stream.retry(retryPolicy),
      );

    const snapshot = (serverId: string) =>
      Effect.gen(function* () {
        const [server, channels, members] = yield* Effect.all(
          [
            call((signal) => api.servers.getServer({ serverId }, { signal })),
            call((signal) => api.channels.listChannels({ serverId }, { signal })),
            call((signal) => api.servers.listMembers({ serverId }, { signal })),
          ],
          { concurrency: "unbounded" },
        ).pipe(Effect.retry(retryPolicy));
        store.update((s) => {
          const current = s.instances[key];
          if (!current || !server.server) return s;
          let next = applySnapshot(current, server.server, channels.channels, members.members);
          const focus = s.focus?.instance === key ? s.focus.channel : null;
          for (const event of held.get(serverId) ?? []) next = applyEvent(next, event, focus);
          return { ...s, instances: { ...s.instances, [key]: next } };
        });
        held.delete(serverId);
      }).pipe(
        Effect.catchAll(() =>
          // Left or deleted while loading: stop following it.
          Effect.sync(() => {
            held.delete(serverId);
            cursors.delete(serverId);
            updateInstance(key, (i) => removeServer(i, serverId));
          }).pipe(Effect.zipRight(unfollow(key, serverId))),
        ),
      );

    const handle = (res: SubscribeResponse) =>
      Effect.gen(function* () {
        if (res.ready) {
          for (const head of res.ready.servers) {
            if (cursors.has(head.serverId)) continue;
            cursors.set(head.serverId, head.sequence);
            held.set(head.serverId, []);
            yield* FiberSet.run(snapshots, snapshot(head.serverId));
          }
          patchInstance(key, { connection: "live", problem: null });
        }
        const event = res.event;
        if (!event) return;
        const sid = event.serverId;
        if (event.sequence > 0n) {
          const last = cursors.get(sid);
          if (last !== undefined && event.sequence <= last) return;
          cursors.set(sid, event.sequence);
        }
        const buffer = held.get(sid);
        if (buffer) buffer.push(event);
        else {
          store.update((s) => {
            const current = s.instances[key];
            if (!current) return s;
            const focus = s.focus?.instance === key ? s.focus.channel : null;
            return { ...s, instances: { ...s.instances, [key]: applyEvent(current, event, focus) } };
          });
          onLiveEvent(key, event);
        }
        const me = store.get().instances[key]?.me?.id;
        const gone =
          event.payload.case === "serverDeleted" ||
          (event.payload.case === "memberLeft" && event.payload.value.userId === me);
        if (gone) {
          cursors.delete(sid);
          held.delete(sid);
          yield* unfollow(key, sid);
        }
      });

    yield* followed.changes.pipe(
      Stream.changesWith((a, b) => a.length === b.length && a.every((id, n) => id === b[n])),
      Stream.flatMap((ids) => (ids.length ? subscribe(ids) : Stream.empty), { switch: true }),
      Stream.runForEach(handle),
    );
  });
