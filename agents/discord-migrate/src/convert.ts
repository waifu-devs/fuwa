// Turning Discord's things into fuwa's: permissions, channel order, names and
// message text. Pure, so the tests cover it without either side.
import { Permission } from "@waifu-devs/fuwa";
import type { SourceChannel, SourceEmbed, SourceMessage, SourceOverwrite, TargetOverwrite } from "./model.ts";

/** Discord's permission bits and the fuwa permissions each one becomes. */
const PERMISSIONS: [bigint, Permission[]][] = [
  [1n << 0n, [Permission.CREATE_INVITE]],
  [1n << 1n, [Permission.KICK_MEMBERS]],
  [1n << 2n, [Permission.BAN_MEMBERS]],
  [1n << 3n, [Permission.ADMINISTRATOR]],
  [1n << 4n, [Permission.MANAGE_CHANNELS]],
  [1n << 5n, [Permission.MANAGE_SERVER]],
  [1n << 6n, [Permission.ADD_REACTIONS]],
  [1n << 7n, [Permission.VIEW_AUDIT_LOG]],
  [1n << 9n, [Permission.VIDEO]],
  [1n << 10n, [Permission.VIEW_CHANNELS]],
  // fuwa gave Manage Cards wherever Send Messages was when boards came.
  [1n << 11n, [Permission.SEND_MESSAGES, Permission.MANAGE_CARDS]],
  [1n << 13n, [Permission.MANAGE_MESSAGES]],
  [1n << 14n, [Permission.EMBED_LINKS]],
  [1n << 15n, [Permission.ATTACH_FILES]],
  [1n << 17n, [Permission.MENTION_EVERYONE]],
  [1n << 20n, [Permission.CONNECT]],
  [1n << 21n, [Permission.SPEAK]],
  [1n << 22n, [Permission.MUTE_MEMBERS]],
  [1n << 24n, [Permission.MOVE_MEMBERS]],
  [1n << 26n, [Permission.CHANGE_NICKNAME]],
  [1n << 27n, [Permission.MANAGE_NICKNAMES]],
  [1n << 28n, [Permission.MANAGE_ROLES]],
  [1n << 29n, [Permission.MANAGE_WEBHOOKS]],
  [1n << 30n, [Permission.MANAGE_EMOJI]],
  [1n << 35n, [Permission.CREATE_THREADS]],
  [1n << 40n, [Permission.TIME_OUT_MEMBERS]],
  [1n << 49n, [Permission.CREATE_POLLS]],
];

/** What a channel's overwrites can change in fuwa; the rest is server-wide only. */
const PER_CHANNEL = new Set<Permission>([
  Permission.MANAGE_CHANNELS,
  Permission.VIEW_CHANNELS,
  Permission.SEND_MESSAGES,
  Permission.EMBED_LINKS,
  Permission.ATTACH_FILES,
  Permission.MENTION_EVERYONE,
  Permission.MANAGE_MESSAGES,
  Permission.CREATE_INVITE,
  Permission.CONNECT,
  Permission.SPEAK,
  Permission.MUTE_MEMBERS,
  Permission.MOVE_MEMBERS,
  Permission.VIDEO,
  Permission.RECORD,
  Permission.CREATE_THREADS,
  Permission.CREATE_POLLS,
  Permission.ADD_REACTIONS,
  Permission.MANAGE_CARDS,
]);

/** fuwa's permissions for Discord's bits, in a steady order. */
export function permissionsOf(bits: bigint, channel = false): Permission[] {
  const out = new Set<Permission>();
  for (const [bit, perms] of PERMISSIONS) {
    if ((bits & bit) === 0n) continue;
    for (const p of perms) if (!channel || PER_CHANNEL.has(p)) out.add(p);
  }
  return [...out].sort((a, b) => a - b);
}

/**
 * A channel's overwrites in fuwa: roles by their new ids (@everyone is the
 * server's id). Members' own overwrites are left out, since the people
 * aren't on fuwa yet, and so are roles that weren't copied.
 */
export function overwritesOf(
  overwrites: SourceOverwrite[],
  guildId: string,
  serverId: string,
  roles: Record<string, string>,
): TargetOverwrite[] {
  const out: TargetOverwrite[] = [];
  for (const o of overwrites) {
    if (o.kind !== "role") continue;
    const targetId = o.id === guildId ? serverId : roles[o.id];
    if (!targetId) continue;
    const allow = permissionsOf(o.allow, true);
    const deny = permissionsOf(o.deny, true);
    if (allow.length || deny.length) out.push({ targetId, kind: "role", allow, deny });
  }
  return out;
}

/**
 * Channels as Discord lists them: ones outside a category first, then each
 * category followed by its channels; text-like channels before voice ones,
 * each by position.
 */
export function displayOrder(channels: SourceChannel[]): SourceChannel[] {
  const byPosition = (a: SourceChannel, b: SourceChannel) => a.position - b.position || compareIds(a.id, b.id);
  const voiceLast = (a: SourceChannel, b: SourceChannel) =>
    Number(a.kind === "voice") - Number(b.kind === "voice") || byPosition(a, b);
  const categories = channels.filter((c) => c.kind === "category").sort(byPosition);
  const known = new Set(categories.map((c) => c.id));
  const loose = channels.filter((c) => c.kind !== "category" && !(c.parentId && known.has(c.parentId)));
  const out = loose.sort(voiceLast);
  for (const category of categories) {
    out.push(category, ...channels.filter((c) => c.kind !== "category" && c.parentId === category.id).sort(voiceLast));
  }
  return out;
}

/** Discord's ids are numbers too big for a double: compares two of them. */
export function compareIds(a: string, b: string): number {
  return a.length - b.length || (a < b ? -1 : a > b ? 1 : 0);
}

/** A custom emoji's name as fuwa takes it: 2 to 32 letters, digits and underscores, not already taken. */
export function emojiName(name: string, taken: Set<string>): string {
  let base = name
    .replace(/[^A-Za-z0-9_]/g, "_")
    .replace(/_+/g, "_")
    .replace(/^_|_$/g, "");
  if (base.length < 2) base = `emoji_${base}`.replace(/_$/, "") || "emoji";
  if (base.length < 2) base = "emoji";
  base = base.slice(0, 32);
  let candidate = base;
  for (let n = 2; taken.has(candidate.toLowerCase()); n++) {
    const suffix = `_${n}`;
    candidate = base.slice(0, 32 - suffix.length) + suffix;
  }
  taken.add(candidate.toLowerCase());
  return candidate;
}

/** A name fuwa takes (1 to 100 characters), or the fallback when there's nothing left. */
export function cleanName(name: string, fallback: string, max = 100): string {
  const trimmed = [...name.trim()].slice(0, max).join("");
  return trimmed || fallback;
}

/** What the migration has copied so far, for rewriting what messages point at. */
export interface Mapping {
  guildId: string;
  serverId: string;
  roles: Record<string, string>;
  roleNames: Record<string, string>;
  channels: Record<string, string>;
  channelNames: Record<string, string>;
  emojis: Record<string, { id: string; name: string }>;
}

/**
 * A message's text with Discord's ids made fuwa's: copied roles, channels and
 * emoji by their new ids, people (who aren't on fuwa yet) by name.
 */
export function rewrite(content: string, mentionedUsers: Record<string, string>, map: Mapping): string {
  return content
    .replace(/<@!?(\d+)>/g, (_, id: string) => `@${mentionedUsers[id] ?? "someone"}`)
    .replace(/<@&(\d+)>/g, (_, id: string) => {
      if (id === map.guildId) return "@everyone";
      const to = map.roles[id];
      return to ? `<@&${to}>` : `@${map.roleNames[id] ?? "a role"}`;
    })
    .replace(/<#(\d+)>/g, (_, id: string) => {
      const to = map.channels[id];
      return to ? `<#${to}>` : `#${map.channelNames[id] ?? "a channel"}`;
    })
    .replace(/<(a?):(\w+):(\d+)>/g, (_, animated: string, name: string, id: string) => {
      const to = map.emojis[id];
      return to ? `<${animated}:${to.name}:${to.id}>` : `:${name}:`;
    })
    .replace(/<\/([\w -]+):\d+>/g, (_, command: string) => `\`/${command}\``);
}

/** fuwa's longest message, in characters. */
export const MAX_MESSAGE = 4000;
/** fuwa's most embeds and files on one message. */
export const MAX_EMBEDS = 10;
export const MAX_FILES = 10;

/** Text cut into pieces fuwa takes, at line breaks where it can. */
export function chunks(text: string, max = MAX_MESSAGE): string[] {
  if (!text.trim()) return [];
  const out: string[] = [];
  let rest = [...text];
  while (rest.length > max) {
    let cut = rest.lastIndexOf("\n", max);
    if (cut < max / 2) cut = rest.lastIndexOf(" ", max);
    if (cut < max / 2) cut = max;
    out.push(rest.slice(0, cut).join(""));
    rest = rest.slice(cut);
    if (rest[0] === "\n" || rest[0] === " ") rest = rest.slice(1);
  }
  if (rest.join("").trim()) out.push(rest.join(""));
  return out;
}

/** The first line of a message, short, for a quoted reply. */
function snippet(content: string, max = 100): string {
  const line = content.split("\n").find((l) => l.trim()) ?? "";
  const chars = [...line.replace(/^>+\s*/, "")];
  return chars.length > max ? `${chars.slice(0, max - 1).join("")}…` : chars.join("");
}

/** The text a copied message is posted with: a reply's quote, its words and its stickers. */
export function messageText(message: SourceMessage, map: Mapping): string {
  const parts: string[] = [];
  if (message.replyTo) {
    const quoted = snippet(rewrite(message.replyTo.content, {}, map)) || "a message";
    parts.push(`> ↪ **${message.replyTo.authorName}**: ${quoted}`);
  }
  const body = rewrite(message.content, message.mentionedUsers, map);
  if (body.trim()) parts.push(body);
  if (message.stickers.length) parts.push(message.stickers.map((s) => `*[sticker: ${s}]*`).join(" "));
  return parts.join("\n");
}

/** Embeds fuwa takes: http(s) links only, ten at most. */
export function embedsOf(embeds: SourceEmbed[]): SourceEmbed[] {
  const web = (url?: string) => (url && /^https?:\/\//i.test(url) ? url : undefined);
  return embeds.slice(0, MAX_EMBEDS).map((e) => ({
    title: e.title,
    description: e.description,
    url: web(e.url),
    color: e.color,
    fields: e.fields.slice(0, 25),
    thumbnailUrl: web(e.thumbnailUrl),
    imageUrl: web(e.imageUrl),
  }));
}

/** The day a moment falls on, in UTC, for the dividers between days. */
export function dayOf(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

/** The line between one day's messages and the next: a timestamp everyone reads in their own time zone. */
export function dayDivider(ms: number): string {
  return `── <t:${Math.floor(ms / 1000)}:D> ──`;
}
