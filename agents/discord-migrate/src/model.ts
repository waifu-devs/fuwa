// What a migration reads from Discord and writes to fuwa, as plain data, so
// the migration (migrate.ts) can run against anything shaped like the two
// sides: discord.ts and fuwa.ts for real, fakes in the tests.

/** A Discord server as the bot reads it, before anything is copied. */
export interface SourceGuild {
  id: string;
  name: string;
  description: string;
  iconUrl?: string;
  bannerUrl?: string;
  /** Lowest first; @everyone is the one with `everyone`. */
  roles: SourceRole[];
  channels: SourceChannel[];
  emojis: SourceEmoji[];
  stickers: SourceSticker[];
  sounds: SourceSound[];
}

export interface SourceRole {
  id: string;
  name: string;
  /** 0xRRGGBB, or 0 for none. */
  color: number;
  /** Discord's permission bits. */
  permissions: bigint;
  hoist: boolean;
  mentionable: boolean;
  /** Higher is above. */
  position: number;
  /** A bot's or an integration's role, which Discord keeps for it. */
  managed: boolean;
  everyone: boolean;
}

export type SourceChannelKind = "text" | "announcement" | "voice" | "category" | "forum" | "other";

export interface SourceChannel {
  id: string;
  name: string;
  kind: SourceChannelKind;
  /** The category it sits in. */
  parentId?: string;
  topic: string;
  slowmodeSeconds: number;
  position: number;
  overwrites: SourceOverwrite[];
}

export interface SourceOverwrite {
  /** A role's id (the guild's id for @everyone) or a member's. */
  id: string;
  kind: "role" | "member";
  allow: bigint;
  deny: bigint;
}

export interface SourceEmoji {
  id: string;
  name: string;
  url: string;
  animated: boolean;
}

export interface SourceSticker {
  id: string;
  name: string;
  url: string;
  /** Lottie stickers are drawings fuwa can't show. */
  format: "png" | "apng" | "gif" | "lottie";
}

/** A soundboard sound. */
export interface SourceSound {
  id: string;
  name: string;
  url: string;
  /** From 0 to 1. */
  volume: number;
  /** One of the guild's emoji by id, or a Unicode emoji. */
  emojiId?: string;
  emojiName?: string;
}

/** A thread, or a forum's post (whose first message has the thread's id). */
export interface SourceThread {
  id: string;
  name: string;
}

/** One message from a channel's history. */
export interface SourceMessage {
  id: string;
  authorName: string;
  authorAvatarUrl?: string;
  /** When it was sent, in milliseconds. */
  at: number;
  content: string;
  /** Joins, boosts, pins and the like, which aren't copied. */
  system: boolean;
  pinned: boolean;
  replyTo?: { authorName: string; content: string };
  attachments: SourceAttachment[];
  /** Embeds a bot or webhook sent; link previews are left out (fuwa makes its own). */
  embeds: SourceEmbed[];
  /** Names of the stickers it sent. */
  stickers: string[];
  /** The names of the people it mentions, by id, for `<@id>`. */
  mentionedUsers: Record<string, string>;
  /** The thread started from it, if any. */
  threadId?: string;
  /** Its reactions: a Unicode emoji or a custom one written <:name:id>, and how many. */
  reactions: { emoji: string; count: number }[];
}

export interface SourceAttachment {
  name: string;
  url: string;
  size: number;
  contentType?: string;
  width?: number;
  height?: number;
  spoiler: boolean;
}

export interface SourceEmbed {
  title?: string;
  description?: string;
  url?: string;
  color?: number;
  fields: { name: string; value: string; inline: boolean }[];
  thumbnailUrl?: string;
  imageUrl?: string;
}

/** A file read from Discord. */
export interface Download {
  bytes: Uint8Array;
  contentType: string;
}

/** Discord's side of a migration. */
export interface Source {
  guild(): Promise<SourceGuild>;
  /**
   * The channel's most recent `limit` messages, oldest first, leaving out
   * `after` and everything before it (to pick up where a stopped run left
   * off), and anything sent before `since` (milliseconds since 1970).
   */
  messages(channelId: string, limit: number, after?: string, since?: number): AsyncIterable<SourceMessage>;
  /** A forum's latest `limit` posts that anyone can see, oldest first, leaving out those started before `since`. */
  forumPosts(channelId: string, limit: number, since?: number): Promise<SourceThread[]>;
  /** A file from Discord's CDN, or undefined when it's bigger than `maxBytes`. */
  download(url: string, maxBytes: number): Promise<Download | undefined>;
}

/** A role or channel overwrite as fuwa takes it: fuwa's permission numbers. */
export interface TargetOverwrite {
  targetId: string;
  kind: "role" | "member";
  allow: number[];
  deny: number[];
}

export interface TargetPost {
  content: string;
  username: string;
  avatarUrl?: string;
  embeds: SourceEmbed[];
}

export interface TargetFile {
  name: string;
  bytes: Uint8Array;
  contentType: string;
  width?: number;
  height?: number;
  spoiler: boolean;
}

/** A webhook the migration posts history through, one per channel. */
export interface Hook {
  id: string;
  channelId: string;
}

/**
 * One thing can't be copied, for good: fuwa turned it down (AutoMod, a limit,
 * something it doesn't take) or Discord won't let the bot read it. The
 * migration skips it and goes on. Any other failure stops the run, since it
 * would fail the same way for everything after it.
 */
export class Refused extends Error {}

/** fuwa's side of a migration: one server, through the agent. */
export interface Target {
  serverId: string;
  /** What the agent lacks to run a migration there; empty when it can. */
  missing(): Promise<string[]>;
  updateServer(change: { name: string; description: string; icon?: Download; banner?: Download }): Promise<void>;
  createRole(role: {
    name: string;
    color?: number;
    permissions: number[];
    hoist: boolean;
    mentionable: boolean;
  }): Promise<string>;
  /** @everyone's permissions. */
  setEveryone(permissions: number[]): Promise<void>;
  /** Puts these roles, highest first, at the top of what the agent may move. */
  orderRoles(roleIds: string[]): Promise<void>;
  createChannel(channel: {
    name: string;
    kind: "text" | "announcement" | "voice" | "category";
    parentId?: string;
    topic: string;
    slowmodeSeconds: number;
    overwrites: TargetOverwrite[];
  }): Promise<string>;
  /** Puts these channels first, in this order, each in its category; the server's others follow. */
  orderChannels(channels: { id: string; parentId?: string }[]): Promise<void>;
  createEmoji(name: string, picture: Download): Promise<{ id: string; name: string }>;
  createHook(channelId: string, name: string): Promise<Hook>;
  deleteHook(hook: Hook): Promise<void>;
  /**
   * Posts through the hook, in order, in as few calls as the instance allows,
   * and returns each message's id, or why fuwa turned that one down.
   */
  postMany(hook: Hook, posts: TargetPost[]): Promise<(string | Refused)[]>;
  /** Sends files as the agent and returns the message's id. */
  sendFiles(channelId: string, content: string, files: TargetFile[]): Promise<string>;
  /** Replies as the agent in the thread under a message, starting it if there's none yet. */
  reply(channelId: string, threadId: string, content: string, files: TargetFile[]): Promise<string>;
  /** Whether the instance has a soundboard (docs/soundboard.md); older ones don't. */
  soundboard(): Promise<boolean>;
  /** Adds a soundboard sound from a file in any format (turned into Ogg Opus here). */
  createSound(sound: { name: string; emoji: string; volume: number; file: Download }): Promise<void>;
  pin(channelId: string, messageId: string): Promise<void>;
  /** An invite to the server that never runs out, as a link. */
  createInvite(): Promise<string>;
}
