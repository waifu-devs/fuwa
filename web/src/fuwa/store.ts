import { useSyncExternalStore } from "react";
import type {
  Application,
  Attachment,
  Channel,
  Emoji,
  Event,
  Member,
  Message,
  Node,
  NotificationSettings,
  Poll,
  Profile,
  Role,
  Server,
  SignInMethod,
  User,
  VoiceState,
  LiveTile,
} from "@/gen/fuwa/v1/types_pb";
import { equals } from "@bufbuild/protobuf";
import { ApplicationStatus, ChannelType, UserSchema } from "@/gen/fuwa/v1/types_pb";
import type { DmCall } from "@/gen/fuwa/v1/call_pb";
import type { ListConnectionsResponse } from "@/gen/fuwa/v1/channel_pb";
import type { Conversation } from "@/gen/fuwa/v1/dm_pb";
import type { Item } from "@/e2ee/vault";
import type { Applied } from "@/lib/applied";
import { emptyFriends, type FriendsState } from "@/lib/friends";
import type { RailLayout } from "@/lib/rail";
import { sortRoles } from "@/lib/permissions";

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
export type PendingMessage = {
  nonce: string;
  content: string;
  /** Files sent with it, already uploaded. */
  files?: Attachment[];
  createdAt: number;
  failed: string | null;
  /** In a secure channel: the thread it's going to, and whether it's also going to the channel. */
  thread?: number;
  inChannel?: boolean;
  /** A voice message on its way: how it looks until it's sent. */
  voice?: { durationMs: number; waveform: Uint8Array };
  /** Files going sealed in an encrypted message: their names and sizes until it's sent. */
  sealed?: { name: string; size: number }[];
};

/** A device in an encrypted conversation, as its group says. */
export type DmMember = { userId: string; deviceId: string; signatureKey: Uint8Array };

/** The account's message backup, as this device takes part in it. */
export type BackupState = {
  /**
   * unknown: not heard yet. unsupported: the instance has no backups. off:
   * the account has none. locked: it has one this device has no key for
   * (enter the recovery key to restore it). on: this device adds to it.
   * restoring: reading it back. full: it reached its size limit.
   */
  status: "unknown" | "unsupported" | "off" | "locked" | "on" | "restoring" | "full";
  problem: string | null;
  size: number;
  maxSize: number;
  /** Unix ms of its last part. */
  updatedAt: number;
  /** While restoring: parts read, of how many. */
  restored: number;
  total: number;
};

export const emptyBackup = (): BackupState => ({
  status: "unknown",
  problem: null,
  size: 0,
  maxSize: 0,
  updatedAt: 0,
  restored: 0,
  total: 0,
});

/** Encrypted direct messages on one instance, as this browser's device sees them. */
export type DmState = {
  /**
   * starting: loading the encryption and registering this device. ready:
   * working. unsupported: this browser can't keep encrypted messages (no
   * IndexedDB or WebAssembly). failed: something else went wrong; see problem.
   */
  status: "off" | "starting" | "ready" | "unsupported" | "failed";
  problem: string | null;
  /** This browser's device for the account. */
  deviceId: string;
  /** The latest first. */
  conversations: Conversation[];
  /** What each conversation said, as this device opened it, oldest first. Only conversations someone opened. */
  items: Record<string, Item[]>;
  pending: Record<string, PendingMessage[]>;
  unread: Record<string, number>;
  /** Every device in each conversation's group. */
  members: Record<string, DmMember[]>;
  /** The safety number as it is now, per conversation. */
  safety: Record<string, string>;
  /** The safety number you checked with the other person. */
  verified: Record<string, string>;
  /** Why you can't send in a conversation right now, if you can't. */
  blocked: Record<string, string>;
  /** Conversations this device is still joining. */
  joining: Record<string, boolean>;
  /** Whether each secure channel passes earlier messages on to people added later, as last heard. */
  secureHistory: Record<string, boolean>;
  /** Calls going on in conversations, by conversation id. */
  calls: Record<string, DmCall>;
  backup: BackupState;
  /** Per secure channel: threads followed or unfollowed by hand, and the last reply seen in each (this device only). */
  threadNotes: Record<string, ThreadNote>;
};

/** What this device notes about a secure channel's threads, by their message's record. */
export type ThreadNote = { follows: Record<number, boolean>; read: Record<number, number> };

export const emptyDms = (): DmState => ({
  status: "off",
  problem: null,
  deviceId: "",
  conversations: [],
  items: {},
  pending: {},
  unread: {},
  members: {},
  safety: {},
  verified: {},
  blocked: {},
  joining: {},
  secureHistory: {},
  calls: {},
  backup: emptyBackup(),
  threadNotes: {},
});

export type InstanceState = {
  key: string;
  url: string;
  /** The user id of the account this is for, as kept in this browser; empty while signed out or not known yet. */
  account: string;
  connection: Connection;
  problem: string | null;
  node: Node | null;
  me: User | null;
  admin: boolean;
  /** Ways to sign in added to your account lately, newest first, for the notice every device shows. */
  recentSignIns: SignInMethod[];
  /** Joined servers, in the order they were joined. */
  servers: Server[];
  /** Per server, sorted for the sidebar. */
  channels: Record<string, Channel[]>;
  /** Per server. */
  members: Record<string, Member[]>;
  /** Per server, highest first; @everyone (whose id is the server's) last. */
  roles: Record<string, Role[]>;
  /** Per server, its own emoji, oldest first. */
  emojis: Record<string, Emoji[]>;
  /** Everyone this instance has shown us, by id, so authors resolve even after they leave. */
  users: Record<string, User>;
  /** Per channel, only for channels someone opened; a thread's replies under `threadKey` of the message it's under. */
  messages: Record<string, ChannelMessages>;
  pending: Record<string, PendingMessage[]>;
  /** Per channel: messages from others that arrived while it wasn't open. */
  unread: Record<string, number>;
  /** The messages threads opened here are under, by id, with their summaries. */
  threadParents: Record<string, Message>;
  /** Per server, once loaded: the threads you follow, by the id of the message each is under. */
  followed: Record<string, Record<string, true>>;
  /** Per thread you follow: replies from others that came while it wasn't open. */
  threadUnread: Record<string, number>;
  /** Servers whose channels and members are loaded. */
  synced: Record<string, boolean>;
  /** Your notification settings, by `notificationKey`. Only servers and channels that have some. */
  notifications: Record<string, NotificationSettings>;
  /** How you arranged your servers on the rail (kept on your account); null until you do. */
  rail: RailLayout | null;
  /** Profiles looked at, by user id. */
  profiles: Record<string, Profile>;
  /** Per server you can review applications for, once loaded: the ones waiting, oldest first. */
  applications: Record<string, Application[]>;
  /** Servers you applied to and aren't in yet, by server id. Kept in this browser, per account. */
  applied: Record<string, Applied>;
  /** Per server: who's in its voice channels, in the order they joined. */
  voice: Record<string, VoiceState[]>;
  /** Per server: apps' live tiles, as listed when the stream starts and sent since. */
  liveTiles: Record<string, LiveTile[]>;
  /**
   * Per server whose shared channels a manager looked at: its connections,
   * share codes and people kept out. Read again when the server says they changed.
   */
  shared: Record<string, ListConnectionsResponse>;
  dms: DmState;
  /** Your friends, requests and blocks here. */
  friends: FriendsState;
};

/** Where a server's (channel "") or a channel's notification settings are kept. */
export const notificationKey = (serverId: string, channelId = "") => `${serverId}/${channelId}`;

export type FuwaState = {
  instances: Record<string, InstanceState>;
  /** Instance keys, in the order they were added. */
  order: string[];
  /** The channel (or conversation) on screen, and the thread open beside it, so they don't collect unread counts. */
  focus: { instance: string; channel: string; thread?: string } | null;
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

export function emptyInstance(key: string, url: string, account = ""): InstanceState {
  return {
    key,
    url,
    account,
    connection: "connecting",
    problem: null,
    node: null,
    me: null,
    admin: false,
    recentSignIns: [],
    servers: [],
    channels: {},
    members: {},
    roles: {},
    emojis: {},
    users: {},
    messages: {},
    pending: {},
    unread: {},
    threadParents: {},
    followed: {},
    threadUnread: {},
    synced: {},
    notifications: {},
    rail: null,
    profiles: {},
    applications: {},
    applied: {},
    voice: {},
    liveTiles: {},
    shared: {},
    dms: emptyDms(),
    friends: emptyFriends(),
  };
}

/** Changes an instance's direct messages. */
export function updateDms(key: string, fn: (d: DmState, i: InstanceState) => DmState) {
  updateInstance(key, (i) => {
    const dms = fn(i.dms, i);
    return dms === i.dms ? i : { ...i, dms };
  });
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

export const without = <V>(record: Record<string, V>, key: string): Record<string, V> => {
  if (!(key in record)) return record;
  const { [key]: _, ...rest } = record;
  return rest;
};

/** Sidebar order: by position, then creation (ids sort by time). */
export function sortChannels(channels: Channel[]): Channel[] {
  return [...channels].sort((a, b) => a.position - b.position || (a.id < b.id ? -1 : 1));
}

/** By name; lists that rank people group them by role on top of this. */
export function sortMembers(members: Member[]): Member[] {
  return [...members].sort((a, b) =>
    (a.nickname || a.user?.displayName || a.user?.username || "").localeCompare(
      b.nickname || b.user?.displayName || b.user?.username || "",
    ),
  );
}

/**
 * The people a message brings with it: in a shared channel, its author from
 * another server, who isn't a member here, so their name and picture resolve.
 */
export function withSharedAuthors(users: Record<string, User>, messages: (Message | undefined)[]): Record<string, User> {
  let next = users;
  for (const m of messages) {
    const user = m?.shared?.user;
    // Every message carries its own copy: keep the one we have while it says the same, so lists don't redraw.
    if (!user || (next[user.id] && equals(UserSchema, next[user.id]!, user))) continue;
    next = { ...next, [user.id]: user };
  }
  return next;
}

/** Where a thread's replies are kept in `messages` and `pending`. */
export const threadKey = (threadId: string) => `t:${threadId}`;

/** Inserts or replaces a message, keeping the list sorted by id (which is by time). */
export function upsertMessage(items: Message[], message: Message): Message[] {
  const at = items.findIndex((m) => m.id >= message.id);
  if (at === -1) return [...items, message];
  if (items[at]!.id === message.id) {
    // An edit doesn't always say how the thread under it stands; keep what we know.
    const next = !message.thread && items[at]!.thread ? { ...message, thread: items[at]!.thread } : message;
    return items.map((m, i) => (i === at ? keepMyVote(m, next) : m));
  }
  return [...items.slice(0, at), message, ...items.slice(at)];
}

/** A thread's new summary, on the message it's under wherever that's kept. */
export function withThreadSummary(i: InstanceState, channelId: string, threadId: string, thread: Message["thread"]): InstanceState {
  const summary = thread && (thread.replyCount > 0 || thread.locked) ? thread : undefined;
  let next = i;
  const loaded = i.messages[channelId];
  const at = loaded?.items.findIndex((m) => m.id === threadId) ?? -1;
  if (loaded && at !== -1) {
    const items = loaded.items.map((m, n) => (n === at ? { ...m, thread: summary } : m));
    next = { ...next, messages: { ...next.messages, [channelId]: { ...loaded, items } } };
  }
  const parent = i.threadParents[threadId];
  if (parent) next = { ...next, threadParents: { ...next.threadParents, [threadId]: { ...parent, thread: summary } } };
  return next;
}

/**
 * A poll message as it comes again: events and most answers carry everyone's
 * counts but never your own vote, so the vote you had stays.
 */
function keepMyVote(before: Message, after: Message): Message {
  if (!before.poll || !after.poll || after.poll.myAnswerIds.length || !before.poll.myAnswerIds.length) return after;
  return { ...after, poll: { ...after.poll, myAnswerIds: before.poll.myAnswerIds } };
}

/**
 * A poll's new counts on its message, if the channel is loaded. `mine` is your
 * own vote when it's known (your vote's answer, or an event naming you);
 * otherwise the one you had stays.
 */
export function withPoll(i: InstanceState, channelId: string, messageId: string, poll: Poll, mine?: number[]): InstanceState {
  const loaded = i.messages[channelId];
  const at = loaded?.items.findIndex((m) => m.id === messageId) ?? -1;
  if (!loaded || at === -1) return i;
  const message = loaded.items[at]!;
  const next = { ...poll, myAnswerIds: mine ?? message.poll?.myAnswerIds ?? [] };
  const items = loaded.items.map((m, n) => (n === at ? { ...m, poll: next } : m));
  return { ...i, messages: { ...i.messages, [channelId]: { ...loaded, items } } };
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
    roles: without(i.roles, serverId),
    emojis: without(i.emojis, serverId),
    synced: without(i.synced, serverId),
    applications: without(i.applications, serverId),
    voice: without(i.voice, serverId),
    liveTiles: without(i.liveTiles, serverId),
    shared: without(i.shared, serverId),
    messages: keep(i.messages),
    pending: keep(i.pending),
    unread: keep(i.unread),
    followed: without(i.followed, serverId),
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
  roles: Role[],
  emojis: Emoji[] = [],
): InstanceState {
  const next = withChannels(addServer(i, server), server.id, channels);
  return {
    ...next,
    members: { ...next.members, [server.id]: sortMembers(members) },
    roles: { ...next.roles, [server.id]: sortRoles(roles) },
    emojis: { ...next.emojis, [server.id]: emojis },
    users: withUsers(
      next.users,
      members.map((m) => m.user),
    ),
    synced: { ...next.synced, [server.id]: true },
  };
}

/** A server's channels as listed again, letting go of what was in the ones that are gone. */
export function withChannels(i: InstanceState, serverId: string, channels: Channel[]): InstanceState {
  const before = i.channels[serverId] ?? [];
  const kept = new Set(channels.map((c) => c.id));
  let { messages, unread } = i;
  for (const c of before) {
    if (kept.has(c.id)) continue;
    messages = without(messages, c.id);
    unread = without(unread, c.id);
  }
  return { ...i, channels: { ...i.channels, [serverId]: sortChannels(channels) }, messages, unread };
}

/** Applies one event from a server's log. Applying the same event twice changes nothing. */
export function applyEvent(i: InstanceState, event: Event, focusChannel: string | null, focusThread: string | null = null): InstanceState {
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
        voice: i.voice[sid]?.some((v) => v.channelId === id)
          ? { ...i.voice, [sid]: i.voice[sid]!.filter((v) => v.channelId !== id) }
          : i.voice,
        liveTiles: i.liveTiles[sid]?.some((t) => t.channelId === id)
          ? { ...i.liveTiles, [sid]: i.liveTiles[sid]!.filter((t) => t.channelId !== id) }
          : i.liveTiles,
      };
    }
    case "messageCreated":
    case "messageUpdated": {
      const message = p.value.message;
      if (!message) return i;
      const loaded = i.messages[message.channelId];
      const users = withSharedAuthors(i.users, [message]);
      let next = users === i.users ? i : { ...i, users };
      if (message.threadId) {
        const key = threadKey(message.threadId);
        const replies = next.messages[key];
        const seen = replies?.items.some((m) => m.id === message.id);
        if (replies) next = { ...next, messages: { ...next.messages, [key]: { ...replies, items: upsertMessage(replies.items, message) } } };
        if (
          p.case === "messageCreated" &&
          !seen &&
          message.authorId !== i.me?.id &&
          focusThread !== message.threadId &&
          next.followed[sid]?.[message.threadId]
        ) {
          next = { ...next, threadUnread: { ...next.threadUnread, [message.threadId]: (next.threadUnread[message.threadId] ?? 0) + 1 } };
        }
        // Replies stay in their thread unless also sent to the channel.
        if (!message.alsoInChannel) return next;
      }
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
    case "pollUpdated": {
      const { channelId, messageId, poll, voterId, voterAnswerIds } = p.value;
      if (!poll) return i;
      return withPoll(i, channelId, messageId, poll, voterId && voterId === i.me?.id ? voterAnswerIds : undefined);
    }
    case "messageDeleted": {
      const { channelId, messageId } = p.value;
      let next = i;
      // A thread goes with the message it's under; a reply leaves whichever thread holds it.
      if (next.threadParents[messageId] || next.messages[threadKey(messageId)]) {
        next = {
          ...next,
          messages: without(next.messages, threadKey(messageId)),
          pending: without(next.pending, threadKey(messageId)),
          threadParents: without(next.threadParents, messageId),
          threadUnread: without(next.threadUnread, messageId),
        };
      }
      for (const [threadId, parent] of Object.entries(next.threadParents)) {
        const replies = next.messages[threadKey(threadId)];
        if (parent.channelId !== channelId || !replies?.items.some((m) => m.id === messageId)) continue;
        next = {
          ...next,
          messages: { ...next.messages, [threadKey(threadId)]: { ...replies, items: replies.items.filter((m) => m.id !== messageId) } },
        };
      }
      const loaded = next.messages[channelId];
      if (!loaded) return next;
      const items = loaded.items.filter((m) => m.id !== messageId);
      return items.length === loaded.items.length ? next : { ...next, messages: { ...next.messages, [channelId]: { ...loaded, items } } };
    }
    case "threadUpdated":
      return withThreadSummary(i, p.value.channelId, p.value.threadId, p.value.thread);
    case "messagePinned": {
      // Marks the message where it's loaded: in its channel, and in its thread for a reply.
      const { channelId, messageId, threadId, pinnedAt } = p.value;
      let messages = i.messages;
      for (const at of [channelId, threadId && threadKey(threadId)]) {
        const loaded = at ? messages[at] : undefined;
        const found = loaded?.items.find((m) => m.id === messageId);
        if (found) messages = { ...messages, [at!]: { ...loaded!, items: upsertMessage(loaded!.items, { ...found, pinnedAt }) } };
      }
      return messages === i.messages ? i : { ...i, messages };
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
    case "roleCreated":
    case "roleUpdated": {
      const role = p.value.role;
      if (!role) return i;
      const list = (i.roles[sid] ?? []).filter((r) => r.id !== role.id);
      return { ...i, roles: { ...i.roles, [sid]: sortRoles([...list, role]) } };
    }
    case "roleDeleted": {
      // The server takes it from everyone and every channel without saying so for each.
      const id = p.value.roleId;
      const list = i.roles[sid] ?? [];
      if (!list.some((r) => r.id === id)) return i;
      const strip = (m: Member) => (m.roleIds.includes(id) ? { ...m, roleIds: m.roleIds.filter((r) => r !== id) } : m);
      const unwrite = (c: Channel) =>
        c.permissionOverwrites.some((o) => o.targetId === id)
          ? { ...c, permissionOverwrites: c.permissionOverwrites.filter((o) => o.targetId !== id) }
          : c;
      return {
        ...i,
        roles: { ...i.roles, [sid]: list.filter((r) => r.id !== id) },
        members: i.members[sid] ? { ...i.members, [sid]: i.members[sid]!.map(strip) } : i.members,
        channels: i.channels[sid] ? { ...i.channels, [sid]: i.channels[sid]!.map(unwrite) } : i.channels,
      };
    }
    case "emojisUpdated":
      return { ...i, emojis: { ...i.emojis, [sid]: p.value.emojis } };
    // Only a signal: the list is read again where it's kept (see sync.ts).
    case "sharedChannelsUpdated":
      return i;
    case "applicationUpdated": {
      // Only kept for servers whose list someone opened; the rest load fresh.
      const application = p.value.application;
      const list = i.applications[sid];
      if (!application?.user || !list) return i;
      const others = list.filter((a) => a.user?.id !== application.user!.id);
      const next =
        application.status === ApplicationStatus.PENDING
          ? [...others, application].sort((a, b) => Number((a.createdAt?.seconds ?? 0n) - (b.createdAt?.seconds ?? 0n)))
          : others;
      return { ...i, applications: { ...i.applications, [sid]: next }, users: withUsers(i.users, [application.user]) };
    }
    case "voiceStateUpdated": {
      const state = p.value.state;
      return state ? withVoiceState(i, sid, state) : i;
    }
    case "voiceStateRemoved": {
      const list = i.voice[sid] ?? [];
      const { userId, channelId } = p.value;
      const next = list.filter((v) => !(v.userId === userId && (!channelId || v.channelId === channelId)));
      return next.length === list.length ? i : { ...i, voice: { ...i.voice, [sid]: next } };
    }
    case "liveTileUpdated": {
      const tile = p.value.tile;
      return tile ? withLiveTile(i, sid, tile) : i;
    }
    case "liveTileEnded": {
      const list = i.liveTiles[sid] ?? [];
      const { channelId, tileId, sourceId } = p.value;
      const next = list.filter((t) => !(t.channelId === channelId && t.id === tileId && t.sourceId === sourceId));
      return next.length === list.length ? i : { ...i, liveTiles: { ...i.liveTiles, [sid]: next } };
    }
    case "memberLeft": {
      if (p.value.userId === i.me?.id) return removeServer(i, sid);
      // An agent's tiles go with it.
      if (i.liveTiles[sid]?.some((t) => t.sourceId === p.value.userId)) {
        i = { ...i, liveTiles: { ...i.liveTiles, [sid]: i.liveTiles[sid]!.filter((t) => t.sourceId !== p.value.userId) } };
      }
      const list = i.members[sid] ?? [];
      const voice = i.voice[sid]?.some((v) => v.userId === p.value.userId)
        ? { ...i.voice, [sid]: i.voice[sid]!.filter((v) => v.userId !== p.value.userId) }
        : i.voice;
      if (!list.some((m) => m.user?.id === p.value.userId)) return voice === i.voice ? i : { ...i, voice };
      return {
        ...i,
        voice,
        members: { ...i.members, [sid]: list.filter((m) => m.user?.id !== p.value.userId) },
        servers: i.servers.map((s) => (s.id === sid ? { ...s, memberCount: s.memberCount - 1n } : s)),
      };
    }
    default:
      return i;
  }
}

/** Someone joined, moved or changed how they sound: one place per person per server. */
export function withVoiceState(i: InstanceState, serverId: string, state: VoiceState): InstanceState {
  const list = i.voice[serverId] ?? [];
  const at = list.findIndex((v) => v.userId === state.userId);
  const next = at === -1 ? [...list, state] : list.map((v, n) => (n === at ? state : v));
  return { ...i, voice: { ...i.voice, [serverId]: next } };
}

/** An app set or changed a tile: one per app, channel and tile id. */
export function withLiveTile(i: InstanceState, serverId: string, tile: LiveTile): InstanceState {
  const list = i.liveTiles[serverId] ?? [];
  const same = (t: LiveTile) => t.id === tile.id && t.channelId === tile.channelId && t.sourceId === tile.sourceId;
  const at = list.findIndex(same);
  const next = at === -1 ? [...list, tile] : list.map((t, n) => (n === at ? tile : t));
  return { ...i, liveTiles: { ...i.liveTiles, [serverId]: next } };
}

export const isCategory = (c: Channel) => c.type === ChannelType.CATEGORY;
