import { Code } from "@connectrpc/connect";
import { Effect, Fiber, FiberSet, Schedule, Stream, SubscriptionRef } from "effect";
import type { SubscribeResponse } from "@/gen/fuwa/v1/event_pb";
import { ChannelType, type Event } from "@/gen/fuwa/v1/types_pb";
import { dmEngine, startDms, stopDms, wipeDms } from "@/e2ee/engine";
import { onLiveEvent, onRemoved } from "@/lib/notify";
import { fromItems } from "@/lib/rail";
import { reportStartup, reportTiming, type ReportTarget } from "@/lib/reports";
import { makeApi, type Api } from "./client";
import { followFriends } from "./friends";
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
  upsertMessage,
  withChannels,
  withSharedAuthors,
  withUsers,
} from "./store";

/**
 * Keeps every instance in the store in step with its server: loads who you
 * are and which servers you're in, follows all of them over one event
 * stream, and reconnects with backoff, resuming from the last event seen so
 * nothing is missed or applied twice.
 */

/** How often the instance's public details (and its announcement) are read again. */
const NODE_REFRESH = "60 seconds";

/** Retry quickly at first, then every 20 seconds at most. */
const backoff = Schedule.exponential("400 millis", 2).pipe(
  Schedule.union(Schedule.spaced("20 seconds")),
  Schedule.jittered,
);
const retryPolicy = backoff.pipe(Schedule.whileInput((e: FuwaError) => e.retryable));

/** The server sends a heartbeat every 25 seconds; this long without anything means the connection is gone. */
const SILENCE = "70 seconds";

/**
 * A stream that ends without an error: the server stopped (an older one, for a
 * deploy) or everything it followed is gone. Either way, follow again; nothing
 * is shown, since the next try says whether the server is really unreachable.
 */
const ENDED = new FuwaError({ code: Code.Unavailable, message: "the server closed the connection" });

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

/** Where anonymous reports go: the first instance you're signed in to whose telemetry is on. */
export function reportTarget(): ReportTarget | null {
  const s = store.get();
  for (const key of s.order) {
    const e = engines.get(key);
    if (e?.token && s.instances[key]?.node?.telemetry && s.instances[key]?.me) return e.api.node;
  }
  return null;
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
    // Encrypted direct messages run alongside, for as long as this does.
    const token = e.token;
    if (me.user) {
      const user = me.user;
      yield* Effect.acquireRelease(
        Effect.sync(() => startDms(key, api, user, token)),
        () => Effect.sync(() => stopDms(key)),
      );
    }
    // Friends too: listening is also what shows you online to them.
    if (me.user) yield* Effect.forkScoped(followFriends(key, api));
    // Notification settings follow the account; an older instance without them just has none.
    const notifications = yield* call((signal) => api.account.getNotificationSettings({}, { signal })).pipe(
      Effect.map((r) => Object.fromEntries(r.settings.map((n) => [notificationKey(n.serverId, n.channelId), n]))),
      Effect.catchAll((err) => (err.signedOut ? Effect.fail(err) : Effect.succeed({}))),
    );
    patchInstance(key, { notifications });
    // So is how you arranged your servers; an older instance just keeps the order you joined in.
    const rail = yield* call((signal) => api.account.getServerArrangement({}, { signal })).pipe(
      Effect.map((r) => (r.updatedAt ? fromItems(r.items) : null)),
      Effect.catchAll((err) => (err.signedOut ? Effect.fail(err) : Effect.succeed(null))),
    );
    patchInstance(key, { rail });

    const { servers } = yield* retrying(call((signal) => api.servers.listServers({}, { signal })));
    updateInstance(key, (i) => servers.reduce(addServer, i));
    yield* SubscriptionRef.set(
      e.followed,
      servers.map((s) => s.id),
    );
    patchInstance(key, { connection: servers.length ? "connecting" : "live" });

    // The instance's name, sign-up options and announcement change without an event.
    yield* call((signal) => api.node.getNode({}, { signal })).pipe(
      Effect.tap(({ node }) => Effect.sync(() => node && patchInstance(key, { node }))),
      Effect.ignore,
      Effect.repeat(Schedule.spaced(NODE_REFRESH)),
      Effect.delay(NODE_REFRESH),
      Effect.forkScoped,
    );

    yield* followEvents(key, api, e.followed);
  }).pipe(
    Effect.scoped,
    Effect.catchAll((err) =>
      Effect.sync(() => {
        if (err.signedOut) {
          // The session expired or was revoked elsewhere, and its device with it.
          e.token = null;
          persist();
          void wipeDms(key);
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
    /** Where each resumed server's replay started, to tell whether anything happened while away. */
    const resumedFrom = new Map<string, bigint>();
    /** Channel events that arrived while a server's channels were being listed again. */
    const relisting = new Map<string, Event[]>();
    /** When the stream last started, to time how long catching up takes. */
    let subscribedAt = 0;

    const subscribe = (ids: readonly string[]) =>
      Stream.suspend(() => {
        const controller = new AbortController();
        subscribedAt = performance.now();
        const request = { servers: ids.map((serverId) => ({ serverId, afterSequence: cursors.get(serverId) })) };
        resumedFrom.clear();
        for (const [serverId, sequence] of cursors) resumedFrom.set(serverId, sequence);
        return Stream.fromAsyncIterable<SubscribeResponse, FuwaError>(
          api.events.subscribe(request, { signal: controller.signal }),
          toFuwaError,
        ).pipe(
          Stream.concat(Stream.fail(ENDED)),
          Stream.ensuring(Effect.sync(() => controller.abort())),
        );
      }).pipe(
        Stream.timeoutFail(() => new FuwaError({ code: Code.Unavailable, message: "lost the connection" }), SILENCE),
        Stream.tapError((err) =>
          Effect.sync(
            () => err !== ENDED && err.retryable && patchInstance(key, { connection: "reconnecting", problem: err.message }),
          ),
        ),
        Stream.retry(retryPolicy),
      );

    const snapshot = (serverId: string) =>
      Effect.gen(function* () {
        const [server, channels, members, roles, emojis] = yield* Effect.all(
          [
            call((signal) => api.servers.getServer({ serverId }, { signal })),
            call((signal) => api.channels.listChannels({ serverId }, { signal })),
            call((signal) => api.servers.listMembers({ serverId }, { signal })),
            call((signal) => api.roles.listRoles({ serverId }, { signal })),
            // Instances from before custom emoji don't have them.
            call((signal) => api.emojis.listEmojis({ serverId }, { signal })).pipe(
              Effect.catchIf(
                (e) => e.code === Code.Unimplemented,
                () => Effect.succeed({ emojis: [] }),
              ),
            ),
          ],
          { concurrency: "unbounded" },
        ).pipe(Effect.retry(retryPolicy));
        const voice = yield* listVoice(serverId);
        store.update((s) => {
          const current = s.instances[key];
          if (!current || !server.server) return s;
          let next = applySnapshot(current, server.server, channels.channels, members.members, roles.roles, emojis.emojis);
          next = { ...next, voice: { ...next.voice, [serverId]: voice } };
          const focus = s.focus?.instance === key ? s.focus.channel : null;
          const thread = s.focus?.instance === key ? (s.focus.thread ?? null) : null;
          for (const event of held.get(serverId) ?? []) next = applyEvent(next, event, focus, thread);
          return { ...s, instances: { ...s.instances, [key]: next } };
        });
        held.delete(serverId);
        // Secure channels this device is in may have had news while it was away.
        const secure = channels.channels.filter((c) => c.type === ChannelType.SECURE).map((c) => c.id);
        if (secure.length) void dmEngine(key)?.followServer(serverId, secure).catch(() => {});
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

    // Who's in voice channels isn't in the log (it lives only as long as the
    // calls do), so it's listed whenever the stream starts again.
    const listVoice = (serverId: string) =>
      call((signal) => api.calls.listVoiceStates({ serverId }, { signal })).pipe(
        Effect.map((r) => r.states),
        // Instances from before calls, or a server that's mid-move: nobody, for now.
        Effect.orElseSucceed(() => []),
      );
    const relistVoice = (serverId: string) =>
      listVoice(serverId).pipe(
        Effect.tap((states) => Effect.sync(() => updateInstance(key, (i) => (i.synced[serverId] ? { ...i, voice: { ...i.voice, [serverId]: states } } : i)))),
        Effect.asVoid,
      );

    // A replay goes by what you can see now, so channels you gained or lost
    // while away only show up by listing them again.
    const relist = (serverId: string) =>
      Effect.gen(function* () {
        relisting.set(serverId, []);
        const { channels } = yield* call((signal) => api.channels.listChannels({ serverId }, { signal })).pipe(
          Effect.retry(retryPolicy),
        );
        store.update((s) => {
          const current = s.instances[key];
          if (!current?.synced[serverId]) return s;
          let next = withChannels(current, serverId, channels);
          const focus = s.focus?.instance === key ? s.focus.channel : null;
          const thread = s.focus?.instance === key ? (s.focus.thread ?? null) : null;
          for (const event of relisting.get(serverId) ?? []) next = applyEvent(next, event, focus, thread);
          return { ...s, instances: { ...s.instances, [key]: next } };
        });
      }).pipe(
        Effect.ignore,
        Effect.ensuring(Effect.sync(() => relisting.delete(serverId))),
      );

    // A server's shared channels changed: read them again where a manager has them open.
    const relistShared = (serverId: string) =>
      call((signal) => api.shared.listConnections({ serverId }, { signal })).pipe(
        Effect.tap((res) =>
          Effect.sync(() =>
            updateInstance(key, (i) =>
              i.shared[serverId]
                ? { ...i, shared: { ...i.shared, [serverId]: res }, users: withUsers(i.users, res.blocks.map((b) => b.user)) }
                : i,
            ),
          ),
        ),
        Effect.ignore,
      );

    // What's said in a channel shown from another server arrives live but isn't
    // in this server's log, so after a gap the channels open here read their
    // latest messages again from the home.
    const rereadShown = (serverId: string) =>
      Effect.forEach(
        (store.get().instances[key]?.channels[serverId] ?? []).filter(
          (c) => c.shared && !c.shared.home && store.get().instances[key]?.messages[c.id],
        ),
        (channel) =>
          call((signal) => api.messages.listMessages({ serverId, channelId: channel.id, limit: 50 }, { signal })).pipe(
            Effect.tap((res) =>
              Effect.sync(() =>
                updateInstance(key, (i) => {
                  const loaded = i.messages[channel.id];
                  if (!loaded) return i;
                  return {
                    ...i,
                    users: withSharedAuthors(withUsers(i.users, res.authors), res.messages),
                    messages: { ...i.messages, [channel.id]: { ...loaded, items: res.messages.reduce(upsertMessage, loaded.items) } },
                  };
                }),
              ),
            ),
            Effect.ignore,
          ),
        { concurrency: 2, discard: true },
      );

    const handle = (res: SubscribeResponse) =>
      Effect.gen(function* () {
        if (res.ready) {
          for (const head of res.ready.servers) {
            if (cursors.has(head.serverId)) {
              yield* FiberSet.run(snapshots, relistVoice(head.serverId));
              yield* FiberSet.run(snapshots, rereadShown(head.serverId));
              const from = resumedFrom.get(head.serverId);
              if (from !== undefined && head.sequence > from) yield* FiberSet.run(snapshots, relist(head.serverId));
              continue;
            }
            cursors.set(head.serverId, head.sequence);
            held.set(head.serverId, []);
            yield* FiberSet.run(snapshots, snapshot(head.serverId));
          }
          patchInstance(key, { connection: "live", problem: null });
          reportTiming("catch_up", performance.now() - subscribedAt);
          reportStartup();
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
        const me = store.get().instances[key]?.me?.id;
        const removed =
          event.payload.case === "memberLeft" && event.payload.value.userId === me
            ? { name: store.get().instances[key]?.servers.find((s) => s.id === sid)?.name, reason: event.payload.value.reason }
            : null;
        if (buffer) buffer.push(event);
        else {
          store.update((s) => {
            const current = s.instances[key];
            if (!current) return s;
            const focus = s.focus?.instance === key ? s.focus.channel : null;
            const thread = s.focus?.instance === key ? (s.focus.thread ?? null) : null;
            return { ...s, instances: { ...s.instances, [key]: applyEvent(current, event, focus, thread) } };
          });
          onLiveEvent(key, event);
          dmEngine(key)?.onServerEvent(event);
          const kind = event.payload.case;
          if (kind === "channelCreated" || kind === "channelUpdated" || kind === "channelDeleted") relisting.get(sid)?.push(event);
          if (kind === "sharedChannelsUpdated" && store.get().instances[key]?.shared[sid]) yield* FiberSet.run(snapshots, relistShared(sid));
        }
        if (removed?.name) onRemoved(removed.name, removed.reason);
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
