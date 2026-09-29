import { useSyncExternalStore } from "react";
import type { Channel, Event, Member, Message, Node, NotificationSettings, Profile, Server, User } from "@/gen/fuwa/v1/types_pb";
import { ChannelType } from "@/gen/fuwa/v1/types_pb";

/**
 * Everything the client shows, for every instance at once. It changes only
 * through `update`, always by replacing objects (never mutating), so React
 * components can select pieces of it and re-render only when those change.
 */

export type Connection = "connecting" | "live" | "reconnecting" | "offline" | "signed-out";

export type ChannelMessages = {
  /** Oldest first. */
  items: Message[];
  /** Older messages exist on the server. */
  hasMore: boolean;
  loading: boolean;
};

/** A message this browser sent that the server hasn't confirmed yet. */
export type PendingMessage = { nonce: string; content: string; createdAt: number; failed: string | null };

export type InstanceState = {
  key: string;
  url: string;
  connection: Connection;
  problem: string | null;
  node: Node | null;
  me: User | null;
  admin: boolean;
  /** Joined servers, in the order they were joined. */
  servers: Server[];
  /** Per server, sorted for the sidebar. */
  channels: Record<string, Channel[]>;
  /** Per server. */
  members: Record<string, Member[]>;
  /** Everyone this instance has shown us, by id, so authors resolve even after they leave. */
  users: Record<string, User>;
  /** Per channel, only for channels someone opened. */
  messages: Record<string, ChannelMessages>;
  pending: Record<string, PendingMessage[]>;
  /** Per channel: messages from others that arrived while it wasn't open. */
  unread: Record<string, number>;
  /** Servers whose channels and members are loaded. */
  synced: Record<string, boolean>;
  /** Your notification settings, by `notificationKey`. Only servers and channels that have some. */
  notifications: Record<string, NotificationSettings>;
  /** Profiles looked at, by user id. */
  profiles: Record<string, Profile>;
};

/** Where a server's (channel "") or a channel's notification settings are kept. */
export const notificationKey = (serverId: string, channelId = "") => `${serverId}/${channelId}`;

export type FuwaState = {
  instances: Record<string, InstanceState>;
  /** Instance keys, in the order they were added. */
  order: string[];
  /** The channel on screen, so it doesn't collect unread counts. */
  focus: { instance: string; channel: string } | null;
};

let state: FuwaState = { instances: {}, order: [], focus: null };
const listeners = new Set<() => void>();

export const store = {
  get: () => state,
  update(fn: (s: FuwaState) => FuwaState) {
    const next = fn(state);
    if (next === state) return;
    state = next;
    for (const l of listeners) l();
  },
  subscribe(listener: () => void) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
};

/** Reads a slice of the state. The selector must return something already in the state (or a primitive). */
export function useFuwa<T>(selector: (s: FuwaState) => T): T {
  return useSyncExternalStore(store.subscribe, () => selector(state));
}

export function emptyInstance(key: string, url: string): InstanceState {
  return {
    key,
    url,
    connection: "connecting",
    problem: null,
    node: null,
    me: null,
    admin: false,
    servers: [],
    channels: {},
    members: {},
    users: {},
    messages: {},
    pending: {},
    unread: {},
    synced: {},
    notifications: {},
    profiles: {},
  };
}

/** Changes one instance; a no-op if it was removed meanwhile. */
export function updateInstance(key: string, fn: (i: InstanceState) => InstanceState) {
  store.update((s) => {
    const current = s.instances[key];
    if (!current) return s;
    const next = fn(current);
    return next === current ? s : { ...s, instances: { ...s.instances, [key]: next } };
  });
}

export function patchInstance(key: string, patch: Partial<InstanceState>) {
  updateInstance(key, (i) => ({ ...i, ...patch }));
}

// ───────────────────────── Pure helpers ─────────────────────────

const without = <V>(record: Record<string, V>, key: string): Record<string, V> => {
  if (!(key in record)) return record;
  const { [key]: _, ...rest } = record;
  return rest;
};

/** Sidebar order: by position, then creation (ids sort by time). */
export function sortChannels(channels: Channel[]): Channel[] {
  return [...channels].sort((a, b) => a.position - b.position || (a.id < b.id ? -1 : 1));
}

const ROLE_RANK = (m: Member) => -m.role;
export function sortMembers(members: Member[]): Member[] {
  return [...members].sort(
    (a, b) =>
      ROLE_RANK(a) - ROLE_RANK(b) ||
      (a.nickname || a.user?.displayName || a.user?.username || "").localeCompare(
        b.nickname || b.user?.displayName || b.user?.username || "",
      ),
  );
}

/** Inserts or replaces a message, keeping the list sorted by id (which is by time). */
export function upsertMessage(items: Message[], message: Message): Message[] {
  const at = items.findIndex((m) => m.id >= message.id);
  if (at === -1) return [...items, message];
  if (items[at]!.id === message.id) return items.map((m, i) => (i === at ? message : m));
  return [...items.slice(0, at), message, ...items.slice(at)];
}

/** Puts a user's new look everywhere it shows: the user list, their memberships and their profile. */
export function withUpdatedUser(i: InstanceState, user: User): InstanceState {
  let members = i.members;
  for (const [serverId, list] of Object.entries(i.members)) {
    if (!list.some((m) => m.user?.id === user.id)) continue;
    members = { ...members, [serverId]: list.map((m) => (m.user?.id === user.id ? { ...m, user } : m)) };
  }
  const profile = i.profiles[user.id];
  return {
    ...i,
    users: withUsers(i.users, [user]),
    members,
    me: i.me?.id === user.id ? user : i.me,
    profiles: profile ? { ...i.profiles, [user.id]: { ...profile, user } } : i.profiles,
  };
}

function withUser(users: Record<string, User>, user: User | undefined): Record<string, User> {
  if (!user) return users;
  return users[user.id] === user ? users : { ...users, [user.id]: user };
}

export function withUsers(users: Record<string, User>, list: (User | undefined)[]): Record<string, User> {
  return list.reduce(withUser, users);
}

export function removeServer(i: InstanceState, serverId: string): InstanceState {
  const channelIds = new Set((i.channels[serverId] ?? []).map((c) => c.id));
  const keep = <V>(r: Record<string, V>) => Object.fromEntries(Object.entries(r).filter(([k]) => !channelIds.has(k)));
  return {
    ...i,
    servers: i.servers.filter((s) => s.id !== serverId),
    channels: without(i.channels, serverId),
    members: without(i.members, serverId),
    synced: without(i.synced, serverId),
    messages: keep(i.messages),
    pending: keep(i.pending),
    unread: keep(i.unread),
  };
}

export function addServer(i: InstanceState, server: Server): InstanceState {
  const at = i.servers.findIndex((s) => s.id === server.id);
  return {
    ...i,
    servers: at === -1 ? [...i.servers, server] : i.servers.map((s, n) => (n === at ? server : s)),
  };
}

/** A server's state as loaded in one go, after the event stream said where it stands. */
export function applySnapshot(
  i: InstanceState,
  server: Server,
  channels: Channel[],
  members: Member[],
): InstanceState {
  const next = addServer(i, server);
  return {
    ...next,
    channels: { ...next.channels, [server.id]: sortChannels(channels) },
    members: { ...next.members, [server.id]: sortMembers(members) },
    users: withUsers(
      next.users,
      members.map((m) => m.user),
    ),
    synced: { ...next.synced, [server.id]: true },
  };
}

/** Applies one event from a server's log. Applying the same event twice changes nothing. */
export function applyEvent(i: InstanceState, event: Event, focusChannel: string | null): InstanceState {
  const sid = event.serverId;
  const p = event.payload;
  switch (p.case) {
    case "serverUpdated": {
      const server = p.value.server;
      return server && i.servers.some((s) => s.id === server.id) ? addServer(i, server) : i;
    }
    case "serverDeleted":
      return removeServer(i, sid);
    case "channelCreated":
    case "channelUpdated": {
      const channel = p.value.channel;
      if (!channel) return i;
      const list = (i.channels[sid] ?? []).filter((c) => c.id !== channel.id);
      return { ...i, channels: { ...i.channels, [sid]: sortChannels([...list, channel]) } };
    }
    case "channelDeleted": {
      const id = p.value.channelId;
      return {
        ...i,
        channels: { ...i.channels, [sid]: (i.channels[sid] ?? []).filter((c) => c.id !== id) },
        messages: without(i.messages, id),
        unread: without(i.unread, id),
      };
    }
    case "messageCreated":
    case "messageUpdated": {
      const message = p.value.message;
      if (!message) return i;
      const loaded = i.messages[message.channelId];
      let next = i;
      if (loaded) {
        next = {
          ...next,
          messages: { ...next.messages, [message.channelId]: { ...loaded, items: upsertMessage(loaded.items, message) } },
        };
      }
      const known = loaded?.items.some((m) => m.id === message.id);
      if (
        p.case === "messageCreated" &&
        !known &&
        message.authorId !== i.me?.id &&
        focusChannel !== message.channelId
      ) {
        next = { ...next, unread: { ...next.unread, [message.channelId]: (next.unread[message.channelId] ?? 0) + 1 } };
      }
      return next;
    }
    case "messageDeleted": {
      const loaded = i.messages[p.value.channelId];
      if (!loaded) return i;
      const items = loaded.items.filter((m) => m.id !== p.value.messageId);
      return items.length === loaded.items.length
        ? i
        : { ...i, messages: { ...i.messages, [p.value.channelId]: { ...loaded, items } } };
    }
    case "userUpdated": {
      const user = p.value.user;
      return user ? withUpdatedUser(i, user) : i;
    }
    case "memberJoined":
    case "memberUpdated": {
      const member = p.value.member;
      if (!member?.user) return i;
      const list = i.members[sid] ?? [];
      const isNew = !list.some((m) => m.user?.id === member.user!.id);
      const members = sortMembers([...list.filter((m) => m.user?.id !== member.user!.id), member]);
      const profile = i.profiles[member.user.id];
      return {
        ...i,
        members: { ...i.members, [sid]: members },
        users: withUsers(i.users, [member.user]),
        me: i.me?.id === member.user.id ? member.user : i.me,
        profiles: profile ? { ...i.profiles, [member.user.id]: { ...profile, user: member.user } } : i.profiles,
        servers:
          isNew && p.case === "memberJoined"
            ? i.servers.map((s) => (s.id === sid ? { ...s, memberCount: s.memberCount + 1n } : s))
            : i.servers,
      };
    }
    case "memberLeft": {
      if (p.value.userId === i.me?.id) return removeServer(i, sid);
      const list = i.members[sid] ?? [];
      if (!list.some((m) => m.user?.id === p.value.userId)) return i;
      return {
        ...i,
        members: { ...i.members, [sid]: list.filter((m) => m.user?.id !== p.value.userId) },
        servers: i.servers.map((s) => (s.id === sid ? { ...s, memberCount: s.memberCount - 1n } : s)),
      };
    }
    default:
      return i;
  }
}

export const isCategory = (c: Channel) => c.type === ChannelType.CATEGORY;
