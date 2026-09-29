import { Effect } from "effect";
import type { InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import { ChannelType, type Server, type ServerLimits } from "@/gen/fuwa/v1/types_pb";
import { makeApi } from "./client";
import { call, toFuwaError, type FuwaError } from "./errors";
import { normalizeUrl } from "./saved";
import { addInstance, engine, follow, removeInstance } from "./sync";
import { addServer, removeServer, store, updateInstance, upsertMessage, withUsers, type PendingMessage } from "./store";

/**
 * What people do in the client, as Effects. Each one talks to its instance
 * and folds the answer into the store right away, so the screen updates
 * before the matching event comes back over the stream (which then changes
 * nothing, since applying an event twice is harmless).
 */

/** Runs an action from a click or a form. Rejects with a FuwaError. */
export function run<A>(effect: Effect.Effect<A, FuwaError>): Promise<A> {
  return Effect.runPromise(effect.pipe(Effect.mapError(toFuwaError), Effect.either)).then((result) => {
    if (result._tag === "Left") throw result.left;
    return result.right;
  });
}

const api = (key: string) => engine(key).api;

// ───────────────────────── Instances and accounts ─────────────────────────

/** Looks up what an instance is and how you can sign in to it, before adding it. */
export const probe = (input: string) =>
  Effect.gen(function* () {
    const url = yield* Effect.try({
      try: () => normalizeUrl(input),
      catch: () => toFuwaError(new Error("that doesn't look like an address")),
    });
    const { node } = yield* call((signal) => makeApi(url, () => null).node.getNode({}, { signal }));
    return { url, node: node! };
  });

export const signIn = (url: string, username: string, password: string) =>
  Effect.gen(function* () {
    const res = yield* call((signal) => makeApi(url, () => null).auth.signIn({ username, password }, { signal }));
    return addInstance(url, res.token);
  });

export const signUp = (url: string, username: string, password: string, displayName: string) =>
  Effect.gen(function* () {
    const res = yield* call((signal) =>
      makeApi(url, () => null).auth.signUp({ username, password, displayName }, { signal }),
    );
    return addInstance(url, res.token);
  });

/** Ends the session on the server too, then keeps the instance listed but signed out. */
export const signOut = (key: string) =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).auth.signOut({}, { signal })).pipe(Effect.ignore);
    addInstance(engine(key).url, null);
  });

export const forget = (key: string) =>
  Effect.gen(function* () {
    if (engine(key).token) yield* call((signal) => api(key).auth.signOut({}, { signal })).pipe(Effect.ignore);
    removeInstance(key);
  });

export const updateProfile = (key: string, displayName: string, avatarUrl: string) =>
  Effect.gen(function* () {
    const { user } = yield* call((signal) => api(key).auth.updateProfile({ displayName, avatarUrl }, { signal }));
    updateInstance(key, (i) => ({ ...i, me: user ?? i.me, users: withUsers(i.users, [user]) }));
  });

// ───────────────────────── Servers ─────────────────────────

const joined = (key: string, server: Server | undefined) =>
  Effect.gen(function* () {
    if (!server) return;
    updateInstance(key, (i) => addServer(i, server));
    yield* follow(key, server.id);
  });

export const createServer = (key: string, name: string, description: string, discoverable: boolean) =>
  Effect.gen(function* () {
    const { server } = yield* call((signal) =>
      api(key).servers.createServer({ name, description, discoverable }, { signal }),
    );
    yield* joined(key, server);
    return server!;
  });

export const discover = (key: string) =>
  call((signal) => api(key).servers.discoverServers({}, { signal })).pipe(Effect.map((r) => r.servers));

export const joinServer = (key: string, serverId: string) =>
  Effect.gen(function* () {
    const { server } = yield* call((signal) => api(key).servers.joinServer({ serverId }, { signal }));
    yield* joined(key, server);
    return server!;
  });

export const leaveServer = (key: string, serverId: string) =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).servers.leaveServer({ serverId }, { signal }));
    updateInstance(key, (i) => removeServer(i, serverId));
  });

export const deleteServer = (key: string, serverId: string) =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).servers.deleteServer({ serverId }, { signal }));
    updateInstance(key, (i) => removeServer(i, serverId));
  });

export const updateServer = (
  key: string,
  serverId: string,
  patch: { name?: string; description?: string; discoverable?: boolean },
) =>
  Effect.gen(function* () {
    const { server } = yield* call((signal) => api(key).servers.updateServer({ serverId, ...patch }, { signal }));
    if (server) updateInstance(key, (i) => addServer(i, server));
  });

export const serverUsage = (key: string, serverId: string) =>
  call((signal) => api(key).servers.getServerUsage({ serverId }, { signal }));

// ───────────────────────── Instance settings (instance admins) ─────────────────────────

/** The instance's settings, their defaults, which ones were changed, and how it was started. */
export const getSettings = (key: string) =>
  call((signal) => api(key).admin.getSettings({}, { signal })).pipe(Effect.map((r) => r.config!));

/**
 * Changes the settings named in `update` to their values in `settings`, and
 * returns the ones in `reset` to their defaults. The instance's public details
 * are read again, so its name and sign-up options update everywhere.
 */
export const updateSettings = (key: string, settings: InstanceSettings, update: string[], reset: string[]) =>
  Effect.gen(function* () {
    const { config } = yield* call((signal) =>
      api(key).admin.updateSettings(
        { settings, updateMask: { paths: update }, resetMask: { paths: reset } },
        { signal },
      ),
    );
    const { node } = yield* call((signal) => api(key).node.getNode({}, { signal }));
    updateInstance(key, (i) => ({ ...i, node: node ?? i.node }));
    return config!;
  });

export const nodeUsage = (key: string) => call((signal) => api(key).admin.getNodeUsage({}, { signal }));

/** Replaces a server's own caps; unset ones follow the instance defaults. */
export const setServerLimits = (key: string, serverId: string, limits: Omit<ServerLimits, "$typeName">) =>
  call((signal) => api(key).admin.setServerLimits({ serverId, limits }, { signal })).pipe(
    Effect.map((r) => r.limits!),
  );

// ───────────────────────── Channels ─────────────────────────

export const createChannel = (key: string, serverId: string, name: string, type: ChannelType, parentId = "") =>
  Effect.gen(function* () {
    const { channel } = yield* call((signal) =>
      api(key).channels.createChannel({ serverId, name, type, parentId }, { signal }),
    );
    return channel!;
  });

export const updateChannel = (
  key: string,
  serverId: string,
  channelId: string,
  patch: { name?: string; topic?: string },
) => call((signal) => api(key).channels.updateChannel({ serverId, channelId, ...patch }, { signal }));

export const deleteChannel = (key: string, serverId: string, channelId: string) =>
  call((signal) => api(key).channels.deleteChannel({ serverId, channelId }, { signal }));

// ───────────────────────── Messages ─────────────────────────

const PAGE = 50;

/** Loads the latest messages of a channel the first time it's opened, or older ones when scrolling up. */
export const loadMessages = (key: string, serverId: string, channelId: string, older = false) =>
  Effect.gen(function* () {
    const current = store.get().instances[key]?.messages[channelId];
    if (current?.loading || (current && !older) || (older && !current?.hasMore)) return;
    const beforeId = older ? (current?.items[0]?.id ?? "") : "";
    updateInstance(key, (i) => ({
      ...i,
      messages: {
        ...i.messages,
        [channelId]: { ...(i.messages[channelId] ?? { items: [], hasMore: false }), loading: true },
      },
    }));
    const res = yield* call((signal) =>
      api(key).messages.listMessages({ serverId, channelId, limit: PAGE, beforeId }, { signal }),
    ).pipe(
      Effect.tapError(() =>
        Effect.sync(() =>
          updateInstance(key, (i) => {
            const { [channelId]: _, ...messages } = i.messages;
            return { ...i, messages: current ? { ...messages, [channelId]: { ...current, loading: false } } : messages };
          }),
        ),
      ),
    );
    updateInstance(key, (i) => {
      const existing = i.messages[channelId]?.items ?? [];
      const items = res.messages.reduce(upsertMessage, existing);
      return {
        ...i,
        users: withUsers(i.users, res.authors),
        messages: {
          ...i.messages,
          [channelId]: { items, hasMore: older || !current ? res.hasMore : (current?.hasMore ?? false), loading: false },
        },
      };
    });
  });

let nonce = 0;

/** Sends a message. It shows up right away, dimmed until the server confirms it. */
export const sendMessage = (key: string, serverId: string, channelId: string, content: string) =>
  Effect.gen(function* () {
    const pending: PendingMessage = { nonce: `n${++nonce}`, content, createdAt: Date.now(), failed: null };
    const setPending = (fn: (list: PendingMessage[]) => PendingMessage[]) =>
      updateInstance(key, (i) => ({ ...i, pending: { ...i.pending, [channelId]: fn(i.pending[channelId] ?? []) } }));
    setPending((list) => [...list, pending]);
    const res = yield* call((signal) => api(key).messages.sendMessage({ serverId, channelId, content }, { signal })).pipe(
      Effect.tapError((err) =>
        Effect.sync(() =>
          setPending((list) => list.map((p) => (p.nonce === pending.nonce ? { ...p, failed: err.message } : p))),
        ),
      ),
    );
    updateInstance(key, (i) => {
      const loaded = i.messages[channelId];
      return {
        ...i,
        pending: { ...i.pending, [channelId]: (i.pending[channelId] ?? []).filter((p) => p.nonce !== pending.nonce) },
        messages:
          loaded && res.message
            ? { ...i.messages, [channelId]: { ...loaded, items: upsertMessage(loaded.items, res.message) } }
            : i.messages,
      };
    });
  });

export const dismissPending = (key: string, channelId: string, pendingNonce: string) =>
  updateInstance(key, (i) => ({
    ...i,
    pending: { ...i.pending, [channelId]: (i.pending[channelId] ?? []).filter((p) => p.nonce !== pendingNonce) },
  }));

export const editMessage = (key: string, serverId: string, channelId: string, messageId: string, content: string) =>
  Effect.gen(function* () {
    const { message } = yield* call((signal) =>
      api(key).messages.updateMessage({ serverId, messageId, content }, { signal }),
    );
    updateInstance(key, (i) => {
      const loaded = i.messages[channelId];
      if (!loaded || !message) return i;
      return { ...i, messages: { ...i.messages, [channelId]: { ...loaded, items: upsertMessage(loaded.items, message) } } };
    });
  });

export const deleteMessage = (key: string, serverId: string, channelId: string, messageId: string) =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).messages.deleteMessage({ serverId, messageId }, { signal }));
    updateInstance(key, (i) => {
      const loaded = i.messages[channelId];
      if (!loaded) return i;
      return {
        ...i,
        messages: { ...i.messages, [channelId]: { ...loaded, items: loaded.items.filter((m) => m.id !== messageId) } },
      };
    });
  });

/** Marks a channel as the one on screen and clears its unread count. */
export function focusChannel(key: string | null, channelId: string | null) {
  store.update((s) => {
    const focus = key && channelId ? { instance: key, channel: channelId } : null;
    let next = s.focus?.instance === focus?.instance && s.focus?.channel === focus?.channel ? s : { ...s, focus };
    const inst = key ? next.instances[key] : undefined;
    if (inst && channelId && inst.unread[channelId]) {
      const { [channelId]: _, ...unread } = inst.unread;
      next = { ...next, instances: { ...next.instances, [key!]: { ...inst, unread } } };
    }
    return next;
  });
}

export { ChannelType };
