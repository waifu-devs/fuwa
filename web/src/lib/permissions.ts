import {
  ChannelType,
  OverwriteTarget,
  Permission as P,
  type Channel,
  type Member,
  type PermissionOverwrite,
  type Role,
} from "@/gen/fuwa/v1/types_pb";
import type { Key } from "@/i18n/i18n";

/**
 * Roles and permissions: what a member of a community server can do,
 * server-wide and in each channel. Worked out exactly as the server does in
 * `server/src/permissions.rs`, so the app only offers what will work.
 */

/** A set of permissions: bit `1 << n` for each `fuwa.v1.Permission` n. */
export type Bits = number;

export const bit = (p: P): Bits => 1 << p;

export const KNOWN: P[] = [
  P.ADMINISTRATOR,
  P.MANAGE_SERVER,
  P.MANAGE_ROLES,
  P.VIEW_AUDIT_LOG,
  P.CHANGE_NICKNAME,
  P.MANAGE_NICKNAMES,
  P.KICK_MEMBERS,
  P.BAN_MEMBERS,
  P.TIME_OUT_MEMBERS,
  P.MANAGE_CHANNELS,
  P.VIEW_CHANNELS,
  P.SEND_MESSAGES,
  P.EMBED_LINKS,
  P.ATTACH_FILES,
  P.MENTION_EVERYONE,
  P.MANAGE_MESSAGES,
  P.CREATE_INVITE,
  P.MANAGE_EMOJI,
  P.MANAGE_WEBHOOKS,
  P.CONNECT,
  P.SPEAK,
  P.MUTE_MEMBERS,
  P.MOVE_MEMBERS,
  P.VIDEO,
  P.RECORD,
  P.CREATE_THREADS,
  P.CREATE_POLLS,
  P.ADD_REACTIONS,
];

export const ALL: Bits = KNOWN.reduce((bits, p) => bits | bit(p), 0);

/** The permissions a channel's overwrites can change. */
export const CHANNEL: Bits = [
  P.MANAGE_CHANNELS,
  P.MANAGE_ROLES,
  P.VIEW_CHANNELS,
  P.SEND_MESSAGES,
  P.CREATE_THREADS,
  P.EMBED_LINKS,
  P.ATTACH_FILES,
  P.MENTION_EVERYONE,
  P.MANAGE_MESSAGES,
  P.CREATE_INVITE,
  P.CONNECT,
  P.SPEAK,
  P.VIDEO,
  P.RECORD,
  P.CREATE_POLLS,
  P.ADD_REACTIONS,
  P.MUTE_MEMBERS,
  P.MOVE_MEMBERS,
].reduce((bits, p) => bits | bit(p), 0);

/** What a member who hasn't agreed to the server's rules yet can't do, as on the server. */
export const TALK: Bits = [
  P.SEND_MESSAGES,
  P.ADD_REACTIONS,
  P.CREATE_THREADS,
  P.CREATE_POLLS,
  P.EMBED_LINKS,
  P.ATTACH_FILES,
  P.MENTION_EVERYONE,
  P.CREATE_INVITE,
  P.CHANGE_NICKNAME,
  P.CONNECT,
  P.SPEAK,
  P.VIDEO,
].reduce(
  (bits, p) => bits | bit(p),
  0,
);

export const fromList = (list: readonly P[]): Bits => list.reduce((bits, p) => (KNOWN.includes(p) ? bits | bit(p) : bits), 0);
export const toList = (bits: Bits): P[] => KNOWN.filter((p) => bits & bit(p));

/** A permission's name, what it does, and (`channel`) what it does in one channel's settings where that reads differently; catalog keys (serversettings.permission.*). */
type PermissionInfo = { label: Key; about: Key; channel?: Key };

/** How a permission reads in the app. */
export const PERMISSIONS: Record<Exclude<P, P.UNSPECIFIED>, PermissionInfo> = {
  [P.ADMINISTRATOR]: { label: "serversettings.permission.administrator", about: "serversettings.permission.administratorAbout" },
  [P.MANAGE_SERVER]: { label: "serversettings.permission.manageServer", about: "serversettings.permission.manageServerAbout" },
  [P.MANAGE_ROLES]: {
    label: "serversettings.permission.manageRoles",
    about: "serversettings.permission.manageRolesAbout",
    channel: "serversettings.permission.manageRolesChannel",
  },
  [P.VIEW_AUDIT_LOG]: { label: "serversettings.permission.viewAuditLog", about: "serversettings.permission.viewAuditLogAbout" },
  [P.CHANGE_NICKNAME]: { label: "serversettings.permission.changeNickname", about: "serversettings.permission.changeNicknameAbout" },
  [P.MANAGE_NICKNAMES]: { label: "serversettings.permission.manageNicknames", about: "serversettings.permission.manageNicknamesAbout" },
  [P.KICK_MEMBERS]: { label: "serversettings.permission.kickMembers", about: "serversettings.permission.kickMembersAbout" },
  [P.BAN_MEMBERS]: { label: "serversettings.permission.banMembers", about: "serversettings.permission.banMembersAbout" },
  [P.TIME_OUT_MEMBERS]: { label: "serversettings.permission.timeOutMembers", about: "serversettings.permission.timeOutMembersAbout" },
  [P.MANAGE_CHANNELS]: {
    label: "serversettings.permission.manageChannels",
    about: "serversettings.permission.manageChannelsAbout",
    channel: "serversettings.permission.manageChannelsChannel",
  },
  [P.VIEW_CHANNELS]: {
    label: "serversettings.permission.viewChannels",
    about: "serversettings.permission.viewChannelsAbout",
    channel: "serversettings.permission.viewChannelsChannel",
  },
  [P.SEND_MESSAGES]: { label: "serversettings.permission.sendMessages", about: "serversettings.permission.sendMessagesAbout" },
  [P.CREATE_THREADS]: {
    label: "serversettings.permission.createThreads",
    about: "serversettings.permission.createThreadsAbout",
    channel: "serversettings.permission.createThreadsChannel",
  },
  [P.CREATE_POLLS]: {
    label: "serversettings.permission.createPolls",
    about: "serversettings.permission.createPollsAbout",
    channel: "serversettings.permission.createPollsChannel",
  },
  [P.ADD_REACTIONS]: {
    label: "serversettings.permission.addReactions",
    about: "serversettings.permission.addReactionsAbout",
    channel: "serversettings.permission.addReactionsChannel",
  },
  [P.EMBED_LINKS]: { label: "serversettings.permission.embedLinks", about: "serversettings.permission.embedLinksAbout" },
  [P.ATTACH_FILES]: { label: "serversettings.permission.attachFiles", about: "serversettings.permission.attachFilesAbout" },
  [P.MENTION_EVERYONE]: { label: "serversettings.permission.mentionEveryone", about: "serversettings.permission.mentionEveryoneAbout" },
  [P.MANAGE_MESSAGES]: { label: "serversettings.permission.manageMessages", about: "serversettings.permission.manageMessagesAbout" },
  [P.CREATE_INVITE]: {
    label: "serversettings.permission.createInvite",
    about: "serversettings.permission.createInviteAbout",
    channel: "serversettings.permission.createInviteChannel",
  },
  [P.MANAGE_EMOJI]: { label: "serversettings.permission.manageEmoji", about: "serversettings.permission.manageEmojiAbout" },
  [P.CONNECT]: { label: "serversettings.permission.connect", about: "serversettings.permission.connectAbout", channel: "serversettings.permission.connectChannel" },
  [P.SPEAK]: { label: "serversettings.permission.speak", about: "serversettings.permission.speakAbout", channel: "serversettings.permission.speakChannel" },
  [P.VIDEO]: { label: "serversettings.permission.video", about: "serversettings.permission.videoAbout", channel: "serversettings.permission.videoChannel" },
  [P.RECORD]: { label: "serversettings.permission.record", about: "serversettings.permission.recordAbout", channel: "serversettings.permission.recordChannel" },
  [P.MUTE_MEMBERS]: {
    label: "serversettings.permission.muteMembers",
    about: "serversettings.permission.muteMembersAbout",
    channel: "serversettings.permission.muteMembersChannel",
  },
  [P.MOVE_MEMBERS]: {
    label: "serversettings.permission.moveMembers",
    about: "serversettings.permission.moveMembersAbout",
    channel: "serversettings.permission.moveMembersChannel",
  },
  [P.MANAGE_WEBHOOKS]: { label: "serversettings.permission.manageWebhooks", about: "serversettings.permission.manageWebhooksAbout" },
};

/** A known permission's catalog keys (every permission the settings pages list is one). */
export const permissionInfo = (p: P): PermissionInfo => PERMISSIONS[p as Exclude<P, P.UNSPECIFIED>];
/** A permission's name in the app's language, or "" for one this app doesn't know. */
export const permissionLabel = (t: (key: Key) => string, p: P) => {
  const info = p === P.UNSPECIFIED ? undefined : PERMISSIONS[p];
  return info ? t(info.label) : "";
};

/** Permissions as the settings pages group them; titles are catalog keys. */
export const PERMISSION_GROUPS: { title: Key; permissions: P[] }[] = [
  {
    title: "serversettings.permission.group.server",
    permissions: [P.VIEW_CHANNELS, P.MANAGE_CHANNELS, P.MANAGE_ROLES, P.MANAGE_EMOJI, P.MANAGE_WEBHOOKS, P.MANAGE_SERVER, P.VIEW_AUDIT_LOG],
  },
  {
    title: "serversettings.permission.group.membership",
    permissions: [P.CREATE_INVITE, P.CHANGE_NICKNAME, P.MANAGE_NICKNAMES, P.KICK_MEMBERS, P.BAN_MEMBERS, P.TIME_OUT_MEMBERS],
  },
  {
    title: "serversettings.permission.group.textChannels",
    permissions: [P.SEND_MESSAGES, P.ADD_REACTIONS, P.CREATE_THREADS, P.CREATE_POLLS, P.EMBED_LINKS, P.ATTACH_FILES, P.MENTION_EVERYONE, P.MANAGE_MESSAGES],
  },
  { title: "serversettings.permission.group.voiceChannels", permissions: [P.CONNECT, P.SPEAK, P.VIDEO, P.RECORD, P.MUTE_MEMBERS, P.MOVE_MEMBERS] },
  { title: "serversettings.permission.group.advanced", permissions: [P.ADMINISTRATOR] },
];

/** The channel permissions, grouped for a channel's settings: everything channels share, then what text and voice channels each have. */
export const CHANNEL_GROUPS: { kind: "general" | "text" | "voice"; title: Key; permissions: P[] }[] = [
  { kind: "general", title: "serversettings.permission.group.general", permissions: [P.VIEW_CHANNELS, P.MANAGE_CHANNELS, P.MANAGE_ROLES, P.CREATE_INVITE] },
  {
    kind: "text",
    title: "serversettings.permission.group.text",
    permissions: [P.SEND_MESSAGES, P.ADD_REACTIONS, P.CREATE_THREADS, P.CREATE_POLLS, P.EMBED_LINKS, P.ATTACH_FILES, P.MENTION_EVERYONE, P.MANAGE_MESSAGES],
  },
  { kind: "voice", title: "serversettings.permission.group.voice", permissions: [P.CONNECT, P.SPEAK, P.VIDEO, P.RECORD, P.MUTE_MEMBERS, P.MOVE_MEMBERS] },
];

/** What one member can do in one server. */
export type Access = {
  owner: boolean;
  /** Server-wide. Every permission for the owner and for administrators. */
  server: Bits;
  /** Their highest role's position; above every role for the owner. */
  rank: number;
  /** Per channel they can see, after its overwrites. */
  channels: ReadonlyMap<string, Bits>;
  /** Hasn't agreed to the server's rules yet, so can read but not talk. */
  pending: boolean;
};

export const NO_ACCESS: Access = { owner: false, server: 0, rank: 0, channels: new Map(), pending: false };

const isRole = (o: PermissionOverwrite) => o.target !== OverwriteTarget.MEMBER;

/** Permissions in a channel: its category's overwrites, then its own. In each, @everyone's, then their roles' together, then their own. */
function channelBits(
  base: Bits,
  everyoneId: string,
  userId: string,
  roleIds: ReadonlySet<string>,
  channel: Channel,
  byId: ReadonlyMap<string, Channel>,
): Bits {
  let bits = base;
  const parent = channel.parentId ? byId.get(channel.parentId) : undefined;
  const layers = parent?.type === ChannelType.CATEGORY ? [parent, channel] : [channel];
  for (const layer of layers) {
    const everyone = layer.permissionOverwrites.find((o) => o.targetId === everyoneId && isRole(o));
    if (everyone) bits = (bits & ~fromList(everyone.deny)) | fromList(everyone.allow);
    let allow = 0;
    let deny = 0;
    for (const o of layer.permissionOverwrites) {
      if (!isRole(o) || !roleIds.has(o.targetId)) continue;
      allow |= fromList(o.allow);
      deny |= fromList(o.deny);
    }
    bits = (bits & ~deny) | allow;
    const own = layer.permissionOverwrites.find((o) => o.targetId === userId && !isRole(o));
    if (own) bits = (bits & ~fromList(own.deny)) | fromList(own.allow);
  }
  return bits & bit(P.VIEW_CHANNELS) ? bits : 0;
}

/**
 * What someone with these roles can do. `everyoneId` is the server's id.
 * Someone `pending` (joined, rules not agreed yet) is held back from talking,
 * and someone `timedOut` can only read until it ends; the owner never is
 * either.
 */
export function accessOf(
  everyoneId: string,
  ownerId: string,
  roles: readonly Role[],
  channels: readonly Channel[],
  userId: string,
  roleIds: readonly string[],
  pending = false,
  timedOut = false,
): Access {
  const owner = userId === ownerId;
  const mine = new Set(roleIds);
  const held = roles.filter((r) => mine.has(r.id));
  const everyone = roles.find((r) => r.id === everyoneId);
  const base = held.reduce((bits, r) => bits | fromList(r.permissions), everyone ? fromList(everyone.permissions) : 0);
  const rank = owner ? Number.MAX_SAFE_INTEGER : Math.max(0, ...held.map((r) => r.position));
  const unbound = owner || !!(base & bit(P.ADMINISTRATOR));
  const byId = new Map(channels.map((c) => [c.id, c]));
  const visible = new Map<string, Bits>();
  for (const c of channels) {
    const bits = unbound ? ALL : channelBits(base, everyoneId, userId, mine, c, byId);
    if (bits & bit(P.VIEW_CHANNELS)) visible.set(c.id, bits);
  }
  // A category shows while any channel in it does.
  for (const c of channels) {
    const parent = c.parentId ? byId.get(c.parentId) : undefined;
    if (visible.has(c.id) && parent?.type === ChannelType.CATEGORY && !visible.has(parent.id)) {
      visible.set(parent.id, bit(P.VIEW_CHANNELS));
    }
  }
  const heldBack = pending && !owner;
  if (heldBack) for (const [id, bits] of visible) visible.set(id, bits & ~TALK);
  if (timedOut && !owner) {
    for (const [id, bits] of visible) visible.set(id, bits & bit(P.VIEW_CHANNELS));
    return { owner, server: (unbound ? ALL : base) & bit(P.VIEW_CHANNELS), rank, channels: visible, pending: heldBack };
  }
  return { owner, server: (unbound ? ALL : base) & (heldBack ? ~TALK : ALL), rank, channels: visible, pending: heldBack };
}

export const has = (a: Access, p: P) => !!(a.server & bit(p));
export const canSee = (a: Access, channelId: string) => a.channels.has(channelId);
export const inChannel = (a: Access, channelId: string) => a.channels.get(channelId) ?? 0;
export const hasIn = (a: Access, channelId: string, p: P) => !!(inChannel(a, channelId) & bit(p));
/** Whether they rank above someone else: the owner above everyone, others by their highest roles. */
export const outranks = (a: Pick<Access, "owner" | "rank">, other: Pick<Access, "owner" | "rank">) =>
  a.owner || (!other.owner && a.rank > other.rank);

/** Where a member stands, for `outranks`, without working out the rest. */
export const standing = (ownerId: string, roles: readonly Role[], member: Member | undefined): Pick<Access, "owner" | "rank"> => {
  const owner = !!member?.user && member.user.id === ownerId;
  return { owner, rank: owner ? Number.MAX_SAFE_INTEGER : Math.max(0, ...rolesOf(roles, member).map((r) => r.position)) };
};
/** Whether they rank above a role at `position`. */
export const above = (a: Access, position: number) => a.owner || a.rank > position;
/** Whether they may grant or take away `changed`, given `have`. */
export const mayChange = (a: Access, changed: Bits, have: Bits) =>
  a.owner || has(a, P.ADMINISTRATOR) || (changed & ~have) === 0;

/** Highest first, as the server lists them. */
export const sortRoles = (roles: readonly Role[]) =>
  [...roles].sort((a, b) => b.position - a.position || (a.id < b.id ? -1 : 1));

/** A member's roles, highest first (without @everyone). */
export const rolesOf = (roles: readonly Role[], member: Pick<Member, "roleIds"> | undefined) => {
  if (!member) return [];
  const mine = new Set(member.roleIds);
  return roles.filter((r) => mine.has(r.id));
};

/** The color a member's name takes: their highest role that has one. */
export const colorOf = (roles: readonly Role[], member: Pick<Member, "roleIds"> | undefined) =>
  rolesOf(roles, member).find((r) => r.color !== undefined)?.color;

/** The role a member is listed under: their highest hoisted one. */
export const hoistedRole = (roles: readonly Role[], member: Pick<Member, "roleIds"> | undefined) =>
  rolesOf(roles, member).find((r) => r.hoist);

/** `0xRRGGBB` as CSS. */
export const cssColor = (color: number) => `#${color.toString(16).padStart(6, "0")}`;

/** The roles a message pings with `<@&id>`. */
export const roleTokens = (content: string): string[] =>
  [...content.matchAll(/<@&([0-9A-Za-z]{26})>/g)].map((m) => m[1]!.toUpperCase());

/** Hidden from @everyone by its own overwrites. */
export const isPrivate = (channel: Channel, everyoneId: string) =>
  channel.permissionOverwrites.some((o) => o.targetId === everyoneId && isRole(o) && o.deny.includes(P.VIEW_CHANNELS));

/** Whether a message says @everyone or @here. */
export const saysEveryone = (content: string) => /(^|[^\w@])@(everyone|here)\b/i.test(content);

/** What a moderator can do to someone: time out, kick, ban, rename. */
export type ModAction = "timeout" | "kick" | "ban" | "nickname";

/** What you may do to someone, from access already worked out: only to people ranked below you, never yourself. */
export function moderationFor(access: Access, ownerId: string, roles: readonly Role[], meId: string | undefined, target: Member | undefined) {
  const below = !!target?.user && target.user.id !== meId && outranks(access, standing(ownerId, roles, target));
  const can = (p: P) => below && has(access, p);
  const allowed: Record<ModAction, boolean> = {
    timeout: can(P.TIME_OUT_MEMBERS),
    kick: can(P.KICK_MEMBERS),
    ban: can(P.BAN_MEMBERS),
    nickname: can(P.MANAGE_NICKNAMES),
  };
  return { ...allowed, any: Object.values(allowed).some(Boolean) };
}
