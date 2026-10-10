import { create } from "@bufbuild/protobuf";
import { Code } from "@connectrpc/connect";
import { Effect, Fiber, FiberSet, Schedule, Stream, SubscriptionRef } from "effect";
import type { SubscribeResponse } from "@/gen/fuwa/v1/event_pb";
import type { WatchFriendsResponse } from "@/gen/fuwa/v1/friend_pb";
import type { ChannelHeads, Focus, OpenResponse } from "@/gen/fuwa/v1/live_pb";
import { ChannelType, EventSchema, type Event, type ProfileItem, type User } from "@/gen/fuwa/v1/types_pb";
import { dmEngine, startDms, stopDms } from "@/e2ee/engine";
import { loadApplied } from "@/lib/applied";
import { adoptInstanceKeys, forgetAccount } from "./accounts";
import { closeNotifications, onLiveEvent, onRemoved } from "@/lib/notify";
import { effectiveNotifications } from "@/lib/notifications";
import { getPrefs } from "@/lib/prefs";
import { clearToasts } from "@/lib/ui";
import { forgetPins, onPinEvent, refreshChannelPins } from "./pins";
import { fromItems } from "@/lib/rail";
import { reportStartup, reportTiming, type ReportTarget } from "@/lib/reports";
import { makeApi, type Api } from "./client";
import { followFriends, friendsLoading, onFriendsResponse, readFriends, setFriends } from "./friends";
import {
  chooseFocus,
  cursorAfterHeads,
  mergeNewest,
  newerHeads,
  queueHead,
  readDone,
  readsDue,
  sameFocus,
  touchedBy,
  wantsEveryMessage,
  withHeads,
  type LiveFocus,
  type NotifyRead,
} from "./live";
import { presenceReader, shownPeople, startPresence, watchShown } from "./presence";
import { FuwaError, call, toFuwaError } from "./errors";
import {
  accountKey,
  activeAccount,
  instanceKey,
  isSavedKey,
  loadSaved,
  savedInstance,
  updateSaved,
  withAccount,
  withActive,
  withCard,
  withInstance,
  withoutAccount,
  withoutInstance,
  replacedTokens,
  type Card,
  type SavedAccount,
} from "./saved";
import { i18n } from "@/i18n/i18n";
import { instanceHas } from "@/lib/compat";
import {
  addServer,
  applyEvent,
  applySnapshot,
  emptyInstance,
  notificationKey,
  patchInstance,
  removeServer,
  store,
  threadKey,
  updateInstance,
  upsertMessage,
  without,
  withChannels,
  withSharedAuthors,
  withUsers,
} from "./store";

/**
 * Keeps every instance in the store in step with its server: loads who you
 * are and which servers you're in, follows all of them over one event
 * stream, and reconnects with backoff, resuming from the last event seen so
 * nothing is missed or applied twice. On instances with live connections
 * (docs/live.md) that stream also carries direct messages, friends and
 * presence, and says what's on screen so busy servers send the rest as heads.
 */

/** How often the instance's public details (and its announcement) are read again. */
const NODE_REFRESH = "60 seconds";

/** A timestamp as a key to compare, "" when unset. */
const stampKey = (at: { seconds: bigint; nanos: number } | undefined) => (at ? `${at.seconds}.${at.nanos}` : "");

/**
 * The instance's profile items (docs/profile-items.md): none on an instance
 * from before them, and null (keep what's there) when listing fails for
 * any other reason than being signed out.
 */
const listProfileItems = (api: Api) =>
  call((signal) => api.profileItems.listInstanceProfileItems({}, { signal })).pipe(
    Effect.map((r) => r.items as ProfileItem[] | null),
    Effect.catchAll((err) => (err.signedOut ? Effect.fail(err) : Effect.succeed(err.code === Code.Unimplemented ? [] : null))),
  );

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
  /** The account the token is for; empty while signed out or before the instance says. */
  userId: string;
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

/** Called before an instance's account goes away (switched, signed out or forgotten), so what it was doing stops. */
const leaving = new Set<(key: string) => void>();
export function onLeaveAccount(fn: (key: string) => void) {
  leaving.add(fn);
  return () => void leaving.delete(fn);
}

function leave(key: string) {
  closeNotifications(key);
  forgetPins(key);
  clearToasts();
  for (const fn of leaving) fn(key);
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

/** Loads the saved instances and starts following them, and follows other tabs switching accounts. Call once at startup. */
export function restore() {
  for (const saved of loadSaved()) start(saved.url, activeAccount(saved) ?? null);
  window.addEventListener("storage", (e) => {
    if (isSavedKey(e.key)) followOtherTabs();
  });
}

/** Another tab changed the kept accounts: run the same account it did on every instance. */
function followOtherTabs() {
  const list = loadSaved();
  for (const saved of list) {
    const key = instanceKey(saved.url);
    const account = activeAccount(saved) ?? null;
    const e = engines.get(key);
    if (!e || e.token !== (account?.token ?? null)) start(saved.url, account);
  }
  for (const key of store.get().order) {
    if (!list.some((i) => instanceKey(i.url) === key)) stop(key);
  }
}

/**
 * (Re)starts an instance as `account`, or signed out: whatever the last
 * account had loaded is dropped, so nothing of it stays on screen or in
 * memory. Returns its key.
 */
function start(url: string, account: SavedAccount | null): string {
  const key = instanceKey(url);
  const existing = engines.get(key);
  if (existing?.fiber) {
    if (existing.userId !== (account?.userId ?? "")) leave(key);
    Effect.runFork(Fiber.interrupt(existing.fiber));
  }
  const e: Engine = existing ?? {
    url,
    api: makeApi(url, () => engines.get(key)?.token ?? null),
    token: null,
    userId: "",
    followed: Effect.runSync(SubscriptionRef.make<readonly string[]>([])),
    fiber: null,
  };
  e.token = account?.token ?? null;
  e.userId = account?.userId ?? "";
  engines.set(key, e);
  store.update((s) => ({
    ...s,
    instances: { ...s.instances, [key]: emptyInstance(key, url, e.userId) },
    order: s.order.includes(key) ? s.order : [...s.order, key],
    focus: s.focus?.instance === key ? null : s.focus,
  }));
  e.fiber = Effect.runFork(run(key, e));
  return key;
}

function stop(key: string) {
  const e = engines.get(key);
  if (e?.fiber) {
    leave(key);
    Effect.runFork(Fiber.interrupt(e.fiber));
  }
  engines.delete(key);
  store.update((s) => {
    const { [key]: _, ...instances } = s.instances;
    return { ...s, instances, order: s.order.filter((k) => k !== key), focus: s.focus?.instance === key ? null : s.focus };
  });
}

const cardOf = (user: User): Card => ({
  userId: user.id,
  username: user.username,
  displayName: user.displayName,
  avatarUrl: user.avatarUrl,
});

/** Signed in as `user`: keeps the account beside any others on the instance, makes it the active one, and starts it. Returns the key. */
export function addAccount(url: string, token: string, user: User | undefined): string {
  if (!user) return addInstance(url);
  let replaced: string[] = [];
  const list = updateSaved((l) => {
    const next = withAccount(l, url, token, cardOf(user));
    replaced = replacedTokens(l, next, url);
    return next;
  });
  endSessions(url, replaced);
  return start(url, activeAccount(savedInstance(list, url)) ?? null);
}

/** Ends sessions this browser let go of for someone still signed in here, so they don't stay live on the instance. */
function endSessions(url: string, tokens: string[]) {
  for (const token of tokens) {
    void makeApi(url, () => token)
      .auth.signOut({})
      .catch(() => {});
  }
}

/** Lists an instance with nobody signed in (or leaves it as it is when it's already listed). Returns its key. */
export function addInstance(url: string): string {
  const key = instanceKey(url);
  updateSaved((l) => withInstance(l, url));
  return engines.has(key) ? key : start(url, null);
}

/** Connects as another account already kept on the instance. */
export function switchAccount(key: string, userId: string) {
  const e = engine(key);
  if (e.userId === userId && e.token) return;
  const list = updateSaved((l) => withActive(l, e.url, userId));
  const account = savedInstance(list, e.url)?.accounts.find((a) => a.userId === userId);
  if (account) start(e.url, account);
}

/** Forgets the instance's active account here and leaves the instance signed out. Returns who it was. */
export function dropAccount(key: string): string {
  const e = engine(key);
  const userId = e.userId;
  const token = e.token;
  updateSaved((l) => {
    const inst = savedInstance(l, e.url);
    // A session kept from before accounts were told apart has no user id yet: go by its token.
    const id = userId || inst?.accounts.find((a) => a.token === token)?.userId;
    return id === undefined ? l : withoutAccount(l, e.url, id);
  });
  start(e.url, null);
  return userId;
}

/** Forgets an instance: stops syncing and drops every account's token from this browser. */
export function removeInstance(key: string) {
  const e = engines.get(key);
  if (e) updateSaved((l) => withoutInstance(l, e.url));
  stop(key);
}

/** The accounts kept on an instance, as cards, for the switcher. */
export function keptAccounts(key: string): SavedAccount[] {
  const e = engines.get(key);
  return e ? (savedInstance(loadSaved(), e.url)?.accounts ?? []) : [];
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
    // Decided from the Node just read, before any feed starts: one live
    // connection for everything, or a stream per feed as before.
    const live = instanceHas(node?.versions, "live-connection");

    const me = yield* retrying(call((signal) => api.auth.getMe({}, { signal })));
    patchInstance(key, { me: me.user ?? null, admin: me.admin, recentSignIns: me.recentSignInMethods });
    if (me.user) {
      const user = me.user;
      const token = e.token;
      e.userId = user.id;
      let fromBefore = false;
      let replaced: string[] = [];
      updateSaved((l) => {
        const { list, wasUnknown } = withCard(l, e.url, token, cardOf(user));
        fromBefore = wasUnknown;
        replaced = replacedTokens(l, list, e.url);
        return list;
      });
      endSessions(e.url, replaced);
      // What this browser kept for the instance before accounts were told apart was this account's.
      if (fromBefore) adoptInstanceKeys(key, user.id);
      patchInstance(key, { account: user.id, applied: loadApplied(accountKey(key, user.id)) });
    }
    // Encrypted direct messages run alongside, for as long as this does.
    const token = e.token;
    if (me.user) {
      const user = me.user;
      yield* Effect.acquireRelease(
        Effect.sync(() => startDms(key, api, user, token, live)),
        () => Effect.sync(() => stopDms(key)),
      );
    }
    // Friends too: listening is also what shows you online to them.
    if (me.user && live) friendsLoading(key);
    else if (me.user) yield* Effect.forkScoped(followFriends(key, api));
    // Presence (who's online, what they're doing) runs alongside too.
    yield* Effect.acquireRelease(
      Effect.sync(() => startPresence(key, api, live)),
      (stop) => Effect.sync(stop),
    );
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
    // The instance's own profile effects and decorations, listed again whenever the Node says they changed.
    let itemsAt = stampKey(store.get().instances[key]?.node?.profileItemsAt);
    const items = yield* listProfileItems(api);
    if (items) patchInstance(key, { profileItems: items });

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
      Effect.tap(({ node }) => {
        const at = stampKey(node?.profileItemsAt);
        if (!node || at === itemsAt) return Effect.void;
        itemsAt = at;
        return listProfileItems(api).pipe(Effect.tap((list) => Effect.sync(() => list && patchInstance(key, { profileItems: list }))));
      }),
      Effect.ignore,
      Effect.repeat(Schedule.spaced(NODE_REFRESH)),
      Effect.delay(NODE_REFRESH),
      Effect.forkScoped,
    );

    yield* followEvents(key, api, e.followed, live ? { person: !!me.user } : null);
  }).pipe(
    Effect.scoped,
    Effect.catchAll((err) =>
      Effect.sync(() => {
        if (err.signedOut) {
          // The session expired or was revoked elsewhere, and its device with it.
          const userId = e.userId;
          const token = e.token;
          e.token = null;
          e.userId = "";
          updateSaved((l) => {
            const id = userId || savedInstance(l, e.url)?.accounts.find((a) => a.token === token)?.userId;
            return id === undefined ? l : withoutAccount(l, e.url, id);
          });
          closeNotifications(key);
          forgetPins(key);
          if (userId) void forgetAccount(key, userId);
          patchInstance(key, { connection: "signed-out", problem: i18n().t("workspace.session.ended") });
        } else {
          patchInstance(key, { connection: "offline", problem: err.message });
        }
      }),
    ),
  );

/** How long the screen settles before a live connection is told what's on it. */
const FOCUS_SETTLE_MS = 150;
/** How long a server added to the followed ones may take to come as `followed` before the live connection opens again. */
const FOLLOW_GRACE = "10 seconds";
/** A channel's newest page, read again when it comes into focus; the same as a channel's first load. */
const PAGE = 50;
/** Messages that came whole kept in mind, so reading a channel for notifications doesn't notify them twice. */
const WHOLE_KEPT = 500;

/**
 * Follows the instance's servers: over EventService.Subscribe, opened again
 * whenever the followed servers change, or with `live` (an instance with
 * live connections) over one LiveService.Open that carries every feed (the
 * friends and direct messages only for a `person`) and is told what's on
 * screen.
 */
const followEvents = (
  key: string,
  api: Api,
  followed: SubscriptionRef.SubscriptionRef<readonly string[]>,
  live: { person: boolean } | null,
) =>
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
    // A live connection's own state, started over each time it opens (see below).
    /** The servers it covers: what it opened with, and what it said it followed since. */
    let covered = new Set<string>();
    /** Its id once it says, for Focus. */
    let connectionId = "";
    /** The channels in focus as it last echoed. */
    let echoed = new Set<string>();
    let readPresence = presenceReader(key);
    /** Messages that came whole lately (see WHOLE_KEPT). */
    const whole = new Set<string>();
    /** Channels whose newest page is being read again, with what came live for them meanwhile. */
    const rereading = new Map<string, { changed: Set<string>; deleted: Set<string> }>();

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
        Stream.timeoutFail(() => new FuwaError({ code: Code.Unavailable, message: i18n().t("system.connection.lost") }), SILENCE),
        Stream.tapError((err) =>
          Effect.sync(
            () => err !== ENDED && err.retryable && patchInstance(key, { connection: "reconnecting", problem: err.message }),
          ),
        ),
        Stream.retry(retryPolicy),
      );

    const snapshot = (serverId: string) =>
      Effect.gen(function* () {
        const [server, channels, members, roles, emojis, profileItems] = yield* Effect.all(
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
            // Nor profile items, from before servers offered them.
            call((signal) => api.profileItems.listServerProfileItems({ serverId }, { signal })).pipe(
              Effect.catchIf(
                (e) => e.code === Code.Unimplemented,
                () => Effect.succeed({ items: [] }),
              ),
            ),
          ],
          { concurrency: "unbounded" },
        ).pipe(Effect.retry(retryPolicy));
        const [voice, tiles] = yield* Effect.all([listVoice(serverId), listTiles(serverId)], { concurrency: "unbounded" });
        store.update((s) => {
          const current = s.instances[key];
          if (!current || !server.server) return s;
          let next = applySnapshot(current, server.server, channels.channels, members.members, roles.roles, emojis.emojis, profileItems.items);
          next = { ...next, voice: { ...next.voice, [serverId]: voice }, liveTiles: { ...next.liveTiles, [serverId]: tiles } };
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
    // Apps' live tiles aren't in the log either: listed with voice, on
    // instances that have them.
    const listTiles = (serverId: string) =>
      Effect.suspend(() =>
        instanceHas(store.get().instances[key]?.node?.versions, "live-tiles")
          ? call((signal) => api.liveTiles.listLiveTiles({ serverId }, { signal })).pipe(Effect.map((r) => r.tiles))
          : Effect.succeed([]),
      ).pipe(Effect.orElseSucceed(() => []));
    const relistTiles = (serverId: string) =>
      listTiles(serverId).pipe(
        Effect.tap((tiles) => Effect.sync(() => updateInstance(key, (i) => (i.synced[serverId] ? { ...i, liveTiles: { ...i.liveTiles, [serverId]: tiles } } : i)))),
        Effect.asVoid,
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
              yield* FiberSet.run(snapshots, relistTiles(head.serverId));
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
          // Out of focus while away, a channel's cached messages missed what
          // came as heads: the one on screen reads its newest page again.
          if (live) for (const channelId of echoed) yield* FiberSet.run(snapshots, reread(channelId));
        }
        // A server joined while the live connection is open: loaded like a
        // new one at `ready`, then what follows applies.
        if (res.followed) {
          const { serverId, sequence } = res.followed;
          covered.add(serverId);
          if (!cursors.has(serverId)) {
            cursors.set(serverId, sequence);
            held.set(serverId, []);
            yield* FiberSet.run(snapshots, snapshot(serverId));
            yield* follow(key, serverId);
          }
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
        if (live && event.payload.case === "messageCreated" && event.payload.value.message) {
          whole.add(event.payload.value.message.id);
          if (whole.size > WHOLE_KEPT) whole.delete(whole.values().next().value!);
        }
        if (buffer) buffer.push(event);
        else {
          store.update((s) => {
            const current = s.instances[key];
            if (!current) return s;
            const focus = s.focus?.instance === key ? s.focus.channel : null;
            const thread = s.focus?.instance === key ? (s.focus.thread ?? null) : null;
            return { ...s, instances: { ...s.instances, [key]: applyEvent(current, event, focus, thread) } };
          });
          // A channel being read again: what this changed wins over the page.
          const touched = live && touchedBy(event);
          const during = touched && rereading.get(touched.channelId);
          if (touched && during) (touched.deleted ? during.deleted : during.changed).add(touched.messageId);
          onLiveEvent(key, event);
          dmEngine(key)?.onServerEvent(event);
          onPinEvent(key, event);
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

    if (!live) {
      yield* followed.changes.pipe(
        Stream.changesWith((a, b) => a.length === b.length && a.every((id, n) => id === b[n])),
        Stream.flatMap((ids) => (ids.length ? subscribe(ids) : Stream.empty), { switch: true }),
        Stream.runForEach(handle),
      );
      return;
    }

    // ─────────────── One live connection (docs/live.md) ───────────────

    // What's on screen, as the connection was last told (in Open or Focus).
    const wanted = (): LiveFocus => chooseFocus(store.get(), key, shownPeople(key));
    /** When to look again for reads for notifications held back by their wait (see below). */
    let notifyWake: ReturnType<typeof setTimeout> | null = null;
    let told: LiveFocus | null = null;
    let telling: ReturnType<typeof setTimeout> | null = null;
    /** The account left (or the instance stopped): nothing more is told or read. */
    let closed = false;
    const tellSoon = (after = FOCUS_SETTLE_MS) => {
      if (!telling && !closed) telling = setTimeout(tell, after);
    };
    // Focus never holds anything up: it's sent on its own, and a connection
    // that's gone meanwhile (NOT_FOUND) opens again with the focus in Open.
    function tell() {
      telling = null;
      const id = connectionId;
      const focus = wanted();
      if (!id || sameFocus(focus, told)) return;
      told = focus;
      api.live.focus({ connectionId: id, focus }).catch((err: unknown) => {
        const e = toFuwaError(err);
        if (closed || e.code === Code.NotFound || e.signedOut || id !== connectionId) return;
        told = null;
        tellSoon(2000);
      });
    }
    yield* Effect.acquireRelease(
      Effect.sync(() => {
        let focus = store.get().focus;
        let channels = store.get().instances[key]?.channels;
        const stopStore = store.subscribe(() => {
          const s = store.get();
          // Only what chooseFocus reads: the channel open, and the channels it may be.
          if (s.focus === focus && s.instances[key]?.channels === channels) return;
          focus = s.focus;
          channels = s.instances[key]?.channels;
          tellSoon();
        });
        const stopShown = watchShown(key, () => tellSoon());
        return () => {
          closed = true;
          connectionId = "";
          stopStore();
          stopShown();
          if (telling) clearTimeout(telling);
          if (notifyWake) clearTimeout(notifyWake);
        };
      }),
      (stop) => Effect.sync(stop),
    );

    const serverOf = (channelId: string) =>
      Object.entries(store.get().instances[key]?.channels ?? {}).find(([, list]) => list.some((c) => c.id === channelId))?.[0];

    // A channel that came into focus (or moved while coming in) reads its
    // newest page again, over what it missed out of focus; a channel never
    // opened just loads when it is. Its threads' replies went stale with it:
    // the one open beside it reads again, the rest load fresh when opened.
    const reread = (channelId: string): Effect.Effect<void> =>
      Effect.suspend(() => {
        const serverId = serverOf(channelId);
        if (!serverId || rereading.has(channelId) || !store.get().instances[key]?.messages[channelId]) return Effect.void;
        const during = { changed: new Set<string>(), deleted: new Set<string>() };
        rereading.set(channelId, during);
        refreshChannelPins(key, serverId, channelId);
        const open = store.get().focus;
        const thread = open?.instance === key && open.channel === channelId ? open.thread : undefined;
        updateInstance(key, (i) => {
          let messages = i.messages;
          for (const [threadId, parent] of Object.entries(i.threadParents)) {
            if (parent.channelId === channelId && threadId !== thread) messages = without(messages, threadKey(threadId));
          }
          return messages === i.messages ? i : { ...i, messages };
        });
        const page = (threadId = "") =>
          call((signal) => api.messages.listMessages({ serverId, channelId, limit: PAGE, threadId }, { signal })).pipe(
            Effect.tap((res) =>
              Effect.sync(() =>
                updateInstance(key, (i) => {
                  const at = threadId ? threadKey(threadId) : channelId;
                  const cached = i.messages[at];
                  if (!cached) return i;
                  const next = {
                    ...i,
                    users: withSharedAuthors(withUsers(i.users, res.authors), [...res.messages, res.parent]),
                    messages: { ...i.messages, [at]: mergeNewest(cached, res, upsertMessage, during) },
                  };
                  return res.parent ? { ...next, threadParents: { ...next.threadParents, [res.parent.id]: res.parent } } : next;
                }),
              ),
            ),
          );
        const thisThread = thread && store.get().instances[key]?.messages[threadKey(thread)] ? page(thread) : Effect.void;
        return Effect.all([page(), thisThread], { concurrency: "unbounded", discard: true }).pipe(
          Effect.ignore,
          Effect.ensuring(Effect.sync(() => rereading.delete(channelId))),
        );
      });

    // The newest message known per channel, from heads and reads (live.ts).
    const known = new Map<string, string>();
    // Channels whose every message notifies, out of focus: what's still to
    // read for notifications (live.ts). Reads are few at once and spaced out
    // per channel; heads that come meanwhile fold into the next one.
    const notifyReads = new Map<string, NotifyRead>();

    const pumpReads = (): Effect.Effect<void> =>
      Effect.gen(function* () {
        if (closed) return;
        if (notifyWake) clearTimeout(notifyWake);
        notifyWake = null;
        const { start, wake } = readsDue(notifyReads, Date.now());
        for (const channelId of start) yield* FiberSet.run(snapshots, readForNotifications(channelId));
        if (wake !== null) notifyWake = setTimeout(() => Effect.runFork(pumpReads()), Math.max(0, wake - Date.now()));
      });

    // What came after the last message read (one page) goes to the notifier
    // as if it had come live. Mentions and your own messages came whole already.
    const readForNotifications = (channelId: string): Effect.Effect<void> =>
      Effect.suspend(() => {
        const r = notifyReads.get(channelId);
        if (!r) return Effect.void;
        r.busy = true;
        r.lastRead = Date.now();
        const head = r.head;
        let read: { newest: string | undefined } | null = null;
        return call((signal) =>
          api.messages.listMessages({ serverId: r.serverId, channelId, limit: PAGE, afterId: r.from ?? "" }, { signal }),
        ).pipe(
          Effect.tap((res) =>
            Effect.sync(() => {
              updateInstance(key, (i) => ({ ...i, users: withSharedAuthors(withUsers(i.users, res.authors), res.messages) }));
              let newest: string | undefined;
              for (const message of [...res.messages].sort((a, b) => (a.id < b.id ? -1 : 1))) {
                newest = message.id;
                if (message.id > (known.get(channelId) ?? "")) known.set(channelId, message.id);
                if (whole.has(message.id)) continue;
                onLiveEvent(
                  key,
                  create(EventSchema, {
                    serverId: r.serverId,
                    createdAt: message.createdAt,
                    payload: { case: "messageCreated", value: { message } },
                  }),
                );
              }
              read = { newest };
            }),
          ),
          Effect.ignore,
          // A read that failed is tried again later from the same place.
          Effect.ensuring(
            Effect.suspend(() => {
              if (read) readDone(notifyReads, channelId, read.newest, head);
              else r.busy = false;
              return pumpReads();
            }),
          ),
        );
      });

    // Where the channels out of focus stand: cursors move on, unread marks
    // light up, and a channel in focus that moved anyway (it came into focus
    // just then) reads its newest page again.
    const onHeads = (heads: ChannelHeads) =>
      Effect.gen(function* () {
        for (const server of heads.servers) {
          const cursor = cursorAfterHeads(cursors.get(server.serverId), server.sequence);
          if (cursor !== undefined) cursors.set(server.serverId, cursor);
        }
        const s = store.get();
        const inst = s.instances[key];
        if (!inst) return;
        const open = s.focus?.instance === key ? s.focus.channel : null;
        const news = newerHeads(inst, heads, known);
        updateInstance(key, (i) => withHeads(i, news, open));
        for (const n of news) {
          known.set(n.channelId, n.lastMessageId);
          if (n.channelId === open || echoed.has(n.channelId)) {
            yield* FiberSet.run(snapshots, reread(n.channelId));
            continue;
          }
          if (wantsEveryMessage(effectiveNotifications(inst, n.serverId, n.channelId), getPrefs().notifyFor)) queueHead(notifyReads, n);
        }
        yield* pumpReads();
      });

    // The focus in effect: channels new to it read what they missed.
    const onFocus = (focus: Focus) =>
      Effect.gen(function* () {
        const before = echoed;
        echoed = new Set(focus.channelIds);
        for (const channelId of echoed) if (!before.has(channelId)) yield* FiberSet.run(snapshots, reread(channelId));
      });

    // Friends at `ready` are listed again without holding up the rest of the
    // connection. Changes that come meanwhile wait for the newest list and
    // apply on top of it; an older list landing late is passed over.
    let friendsReads = 0;
    let friendsHeld: WatchFriendsResponse[] | null = null;
    const relistFriends = (n: number) =>
      readFriends(api).pipe(
        Effect.retry(retryPolicy),
        Effect.tap((read) => Effect.sync(() => n === friendsReads && setFriends(key, read))),
        Effect.ignore,
        Effect.ensuring(
          Effect.suspend(() => {
            if (n !== friendsReads) return Effect.void;
            const held = friendsHeld ?? [];
            friendsHeld = null;
            return Effect.forEach(held, (res) => onFriendsResponse(key, api, res).pipe(Effect.ignore), { discard: true });
          }),
        ),
      );

    const person = live.person;
    const onLive = (res: OpenResponse) =>
      Effect.gen(function* () {
        const item = res.item;
        switch (item.case) {
          case "connectionId":
            connectionId = item.value;
            tellSoon(0);
            return;
          case "events":
            return yield* handle(item.value);
          case "directMessages":
            dmEngine(key)?.deliver(item.value);
            return;
          case "friends":
            if (item.value.ready) {
              friendsHeld ??= [];
              yield* FiberSet.run(snapshots, relistFriends(++friendsReads));
            } else if (friendsHeld) friendsHeld.push(item.value);
            else yield* onFriendsResponse(key, api, item.value).pipe(Effect.ignore);
            return;
          case "presence":
            readPresence(item.value);
            return;
          case "heads":
            return yield* onHeads(item.value);
          case "focus":
            return yield* onFocus(item.value);
        }
        // Nothing set: the heartbeat, which only keeps the silence timeout away.
      });

    const open = Stream.unwrap(
      SubscriptionRef.get(followed).pipe(
        Effect.map((ids) =>
          Stream.suspend(() => {
            const controller = new AbortController();
            subscribedAt = performance.now();
            resumedFrom.clear();
            for (const [serverId, sequence] of cursors) resumedFrom.set(serverId, sequence);
            // Each feed starts over with the connection, as its own stream would.
            covered = new Set(ids);
            connectionId = "";
            readPresence = presenceReader(key);
            told = wanted();
            echoed = new Set(told.channelIds);
            const request = {
              servers: ids.map((serverId) => ({ serverId, afterSequence: cursors.get(serverId) })),
              followNewServers: true,
              directMessages: person,
              friends: person,
              presence: true,
              focus: told,
            };
            return Stream.fromAsyncIterable<OpenResponse, FuwaError>(api.live.open(request, { signal: controller.signal }), toFuwaError).pipe(
              Stream.concat(Stream.fail(ENDED)),
              Stream.ensuring(
                Effect.sync(() => {
                  controller.abort();
                  connectionId = "";
                }),
              ),
            );
          }),
        ),
      ),
    ).pipe(
      Stream.timeoutFail(() => new FuwaError({ code: Code.Unavailable, message: i18n().t("system.connection.lost") }), SILENCE),
      Stream.tapError((err) =>
        Effect.sync(() => err !== ENDED && err.retryable && patchInstance(key, { connection: "reconnecting", problem: err.message })),
      ),
      Stream.retry(retryPolicy),
    );

    // Joining or leaving a server needn't open the connection again: joins
    // come as `followed`, leaving as memberLeft or serverDeleted. Only a
    // server followed here that the connection still doesn't cover after a
    // while (it said nothing of it) does.
    const reopen = yield* SubscriptionRef.make(0);
    const uncovered = (ids: readonly string[]) => ids.some((id) => !covered.has(id));
    yield* followed.changes.pipe(
      Stream.flatMap((ids) => (uncovered(ids) ? Stream.fromEffect(Effect.sleep(FOLLOW_GRACE).pipe(Effect.as(ids))) : Stream.empty), {
        switch: true,
      }),
      Stream.filter(uncovered),
      Stream.runForEach(() => SubscriptionRef.update(reopen, (n) => n + 1)),
      Effect.forkScoped,
    );
    yield* reopen.changes.pipe(
      Stream.flatMap(() => open, { switch: true }),
      Stream.runForEach(onLive),
    );
  });
