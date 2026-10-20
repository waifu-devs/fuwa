// Discord's side of a migration: one guild, read through discord.js with the
// bot's own permissions (View Channels and Read Message History).
import {
  ChannelType,
  type Guild,
  type AnyThreadChannel,
  type GuildBasedChannel,
  type Message,
  OverwriteType,
  PermissionFlagsBits,
  StickerFormatType,
} from "discord.js";
import { compareIds, idAt } from "./convert.ts";
import {
  type Download,
  Refused,
  type Source,
  type SourceChannel,
  type SourceChannelKind,
  type SourceGuild,
  type SourceMessage,
  type SourceSound,
  type SourceThread,
} from "./model.ts";

const PAGE = 100;

export class DiscordSource implements Source {
  readonly #guild: Guild;

  constructor(guild: Guild) {
    this.#guild = guild;
  }

  async guild(): Promise<SourceGuild> {
    const g = this.#guild;
    const [roles, channels, emojis, stickers, sounds] = await Promise.all([
      g.roles.fetch(),
      g.channels.fetch(),
      g.emojis.fetch(),
      g.stickers.fetch(),
      this.#sounds(),
    ]);
    return {
      id: g.id,
      name: g.name,
      description: g.description ?? "",
      iconUrl: g.iconURL({ extension: "png", size: 512 }) ?? undefined,
      bannerUrl: g.bannerURL({ extension: "png", size: 1024 }) ?? undefined,
      roles: [...roles.values()].map((r) => ({
        id: r.id,
        name: r.name,
        color: r.color,
        permissions: r.permissions.bitfield,
        hoist: r.hoist,
        mentionable: r.mentionable,
        position: r.position,
        managed: r.managed,
        everyone: r.id === g.id,
      })),
      channels: [...channels.values()].flatMap((c) => (c ? [channelOf(c)] : [])),
      emojis: [...emojis.values()].map((e) => ({
        id: e.id,
        name: e.name ?? "emoji",
        url: e.imageURL({ extension: e.animated ? "gif" : "png", size: 128 }) ?? "",
        animated: e.animated ?? false,
      })),
      stickers: [...stickers.values()].map((s) => ({
        id: s.id,
        name: s.name,
        url: s.url,
        format:
          s.format === StickerFormatType.Lottie
            ? "lottie"
            : s.format === StickerFormatType.GIF
              ? "gif"
              : s.format === StickerFormatType.APNG
                ? "apng"
                : "png",
      })),
      sounds,
    };
  }

  async #sounds(): Promise<SourceSound[]> {
    const sounds = await this.#guild.soundboardSounds.fetch();
    return [...sounds.values()].map((s) => ({
      id: String(s.soundId),
      name: s.name,
      url: s.url,
      volume: s.volume,
      emojiId: s.emoji?.id ?? undefined,
      emojiName: s.emoji?.name ?? undefined,
    }));
  }

  /** A channel or thread of this guild the bot can read, or Refused. */
  async #readable(channelId: string): Promise<GuildBasedChannel | undefined> {
    const channel = await this.#guild.client.channels.fetch(channelId);
    if (!channel || !("guildId" in channel) || channel.guildId !== this.#guild.id) return undefined;
    const me = this.#guild.members.me;
    if (
      !me ||
      !channel.permissionsFor(me)?.has([PermissionFlagsBits.ViewChannel, PermissionFlagsBits.ReadMessageHistory])
    ) {
      throw new Refused("the bot can't read it on Discord");
    }
    return channel;
  }

  async forumPosts(channelId: string, limit: number, since?: number): Promise<SourceThread[]> {
    const channel = await this.#readable(channelId);
    if (!channel || (channel.type !== ChannelType.GuildForum && channel.type !== ChannelType.GuildMedia)) return [];
    const posts = new Map<string, AnyThreadChannel>();
    for (const [id, t] of (await channel.threads.fetchActive(false)).threads) {
      if (t.parentId === channel.id) posts.set(id, t);
    }
    for (let before: AnyThreadChannel | undefined; ; ) {
      const page = await channel.threads.fetchArchived({ type: "public", before, limit: PAGE }, false);
      for (const [id, t] of page.threads) posts.set(id, t);
      const last = [...page.threads.values()].at(-1);
      if (!page.hasMore || !last) break;
      before = last;
    }
    return [...posts.values()]
      .filter((t) => t.type === ChannelType.PublicThread && (!since || compareIds(t.id, idAt(since)) >= 0))
      .sort((a, b) => compareIds(a.id, b.id))
      .slice(-limit)
      .map((t) => ({ id: t.id, name: t.name }));
  }

  async *messages(channelId: string, limit: number, after?: string, since?: number): AsyncIterable<SourceMessage> {
    const channel = await this.#readable(channelId);
    if (!channel?.isTextBased()) return;
    // Nothing from before `since`: start after the first id that time could have.
    if (since) {
      const floor = (BigInt(idAt(since)) - 1n).toString();
      if (!after || compareIds(after, floor) < 0) after = floor;
    }
    // Walk back to where the window starts, counting only, then read forward
    // a page at a time, so files' links (which Discord signs for a day) are
    // fresh when they're downloaded.
    let start = after ?? "0";
    let counted = 0;
    let before: string | undefined;
    walk: for (;;) {
      const page = await channel.messages.fetch({ limit: PAGE, before, cache: false });
      const ids = [...page.keys()].sort(compareIds).reverse();
      for (const id of ids) {
        if (after && compareIds(id, after) <= 0) break walk;
        if (counted === limit) {
          start = id;
          break walk;
        }
        counted++;
      }
      if (page.size < PAGE) break;
      before = ids[ids.length - 1];
    }
    let given = 0;
    while (given < counted) {
      const page = await channel.messages.fetch({ limit: PAGE, after: start, cache: false });
      const sorted = [...page.values()].sort((a, b) => compareIds(a.id, b.id));
      if (!sorted.length) return;
      for (const message of sorted) {
        if (given === counted) return;
        given++;
        yield await messageOf(message);
      }
      start = sorted[sorted.length - 1]!.id;
    }
  }

  async download(url: string, maxBytes: number): Promise<Download | undefined> {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`Discord answered ${res.status}`);
    const length = Number(res.headers.get("content-length"));
    if (length > maxBytes) {
      await res.body?.cancel();
      return undefined;
    }
    const bytes = new Uint8Array(await res.arrayBuffer());
    if (bytes.byteLength > maxBytes) return undefined;
    return { bytes, contentType: res.headers.get("content-type")?.split(";")[0]?.trim() || "application/octet-stream" };
  }
}

function kindOf(type: ChannelType): SourceChannelKind {
  switch (type) {
    case ChannelType.GuildText:
      return "text";
    case ChannelType.GuildAnnouncement:
      return "announcement";
    case ChannelType.GuildVoice:
    case ChannelType.GuildStageVoice:
      return "voice";
    case ChannelType.GuildCategory:
      return "category";
    case ChannelType.GuildForum:
    case ChannelType.GuildMedia:
      return "forum";
    default:
      return "other";
  }
}

function channelOf(c: GuildBasedChannel): SourceChannel {
  return {
    id: c.id,
    name: c.name,
    kind: kindOf(c.type),
    parentId: c.parentId ?? undefined,
    topic: "topic" in c ? (c.topic ?? "") : "",
    slowmodeSeconds: "rateLimitPerUser" in c ? (c.rateLimitPerUser ?? 0) : 0,
    position: "rawPosition" in c ? c.rawPosition : 0,
    overwrites:
      "permissionOverwrites" in c
        ? [...c.permissionOverwrites.cache.values()].map((o) => ({
            id: o.id,
            kind: o.type === OverwriteType.Role ? "role" : "member",
            allow: o.allow.bitfield,
            deny: o.deny.bitfield,
          }))
        : [],
  };
}

function nameOf(message: Message): string {
  return message.member?.displayName ?? message.author.displayName ?? message.author.username;
}

async function messageOf(message: Message): Promise<SourceMessage> {
  let replyTo: SourceMessage["replyTo"];
  if (message.reference?.messageId && !message.system) {
    // Discord sends the message replied to along, so this is usually cached.
    const replied = await message.fetchReference().catch(() => undefined);
    if (replied) replyTo = { authorName: nameOf(replied), content: replied.content };
  }
  const mentionedUsers: Record<string, string> = {};
  for (const [id, user] of message.mentions.users) {
    mentionedUsers[id] = message.mentions.members?.get(id)?.displayName ?? user.displayName ?? user.username;
  }
  return {
    id: message.id,
    authorName: nameOf(message),
    authorAvatarUrl: (message.member ?? message.author).displayAvatarURL({ extension: "png", size: 128 }),
    at: message.createdTimestamp,
    content: message.content,
    system: message.system,
    pinned: message.pinned,
    replyTo,
    attachments: [...message.attachments.values()].map((a) => ({
      name: a.name,
      url: a.url,
      size: a.size,
      contentType: a.contentType ?? undefined,
      width: a.width ?? undefined,
      height: a.height ?? undefined,
      spoiler: a.spoiler,
    })),
    // Link previews (Discord's "link", "image", "video"...) are left out: fuwa makes its own.
    embeds: message.embeds
      .filter((e) => !e.data.type || e.data.type === "rich")
      .map((e) => ({
        title: e.title ?? undefined,
        description: e.description ?? undefined,
        url: e.url ?? undefined,
        color: e.color ?? undefined,
        fields: e.fields.map((f) => ({ name: f.name, value: f.value, inline: f.inline ?? false })),
        thumbnailUrl: e.thumbnail?.url,
        imageUrl: e.image?.url,
      })),
    stickers: [...message.stickers.values()].map((s) => s.name),
    mentionedUsers,
    // A thread started from a message has the message's id.
    threadId: message.hasThread ? (message.thread?.id ?? message.id) : undefined,
    reactions: [...message.reactions.cache.values()].map((r) => ({
      emoji: r.emoji.id
        ? `<${r.emoji.animated ? "a" : ""}:${r.emoji.name ?? "emoji"}:${r.emoji.id}>`
        : (r.emoji.name ?? "?"),
      count: r.count,
    })),
  };
}
