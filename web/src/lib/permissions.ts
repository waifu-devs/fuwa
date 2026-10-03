import {
  ChannelType,
  OverwriteTarget,
  Permission as P,
  type Channel,
  type Member,
  type PermissionOverwrite,
  type Role,
} from "@/gen/fuwa/v1/types_pb";

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
];

export const ALL: Bits = KNOWN.reduce((bits, p) => bits | bit(p), 0);

/** The permissions a channel's overwrites can change. */
export const CHANNEL: Bits = [
  P.MANAGE_CHANNELS,
  P.MANAGE_ROLES,
  P.VIEW_CHANNELS,
  P.SEND_MESSAGES,
  P.EMBED_LINKS,
  P.ATTACH_FILES,
  P.MENTION_EVERYONE,
  P.MANAGE_MESSAGES,
  P.CREATE_INVITE,
  P.CONNECT,
  P.SPEAK,
  P.VIDEO,
  P.RECORD,
  P.MUTE_MEMBERS,
  P.MOVE_MEMBERS,
].reduce((bits, p) => bits | bit(p), 0);

/** What a member who hasn't agreed to the server's rules yet can't do, as on the server. */
export const TALK: Bits = [
  P.SEND_MESSAGES,
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

type PermissionInfo = { label: string; about: string; channel?: string };

/** How a permission reads in the app. `channel` is its wording in a channel's settings, where it differs. */
export const PERMISSIONS: Record<Exclude<P, P.UNSPECIFIED>, PermissionInfo> = {
  [P.ADMINISTRATOR]: {
    label: "Administrator",
    about: "Every permission, in every channel, whatever a channel says. Still only over roles and people ranked below. Give it with care.",
  },
  [P.MANAGE_SERVER]: {
    label: "Manage server",
    about: "Change the server's name, icon, description, AutoMod and welcome screen, and see its usage. AutoMod leaves them alone.",
  },
  [P.MANAGE_ROLES]: {
    label: "Manage roles",
    about: "Create and edit roles ranked below their own and hand them out, with only the permissions they have.",
    channel: "Change who can do what in this channel.",
  },
  [P.VIEW_AUDIT_LOG]: { label: "View audit log", about: "Read the record of every change made in the server." },
  [P.CHANGE_NICKNAME]: { label: "Change nickname", about: "Set their own nickname in this server." },
  [P.MANAGE_NICKNAMES]: { label: "Manage nicknames", about: "Change the nicknames of people ranked below them." },
  [P.KICK_MEMBERS]: { label: "Kick members", about: "Remove people ranked below them. They can join again." },
  [P.BAN_MEMBERS]: { label: "Ban members", about: "Remove people ranked below them for good, and lift bans." },
  [P.TIME_OUT_MEMBERS]: { label: "Time out members", about: "Stop people ranked below them from talking for a while." },
  [P.MANAGE_CHANNELS]: {
    label: "Manage channels",
    about: "Create, edit, move and delete channels. Also skips slow mode.",
    channel: "Edit or delete this channel. Also skips its slow mode.",
  },
  [P.VIEW_CHANNELS]: {
    label: "View channels",
    about: "See channels and read their messages, unless a channel says otherwise.",
    channel: "See this channel and read its messages.",
  },
  [P.SEND_MESSAGES]: { label: "Send messages", about: "Write in channels." },
  [P.EMBED_LINKS]: { label: "Embed links", about: "Post links." },
  [P.ATTACH_FILES]: { label: "Attach files", about: "Upload files and pictures with their messages." },
  [P.MENTION_EVERYONE]: {
    label: "Mention everyone",
    about: "Ping everyone with @everyone or @here, and any role, even ones that can't be mentioned.",
  },
  [P.MANAGE_MESSAGES]: { label: "Manage messages", about: "Delete other people's messages. Also skips slow mode." },
  [P.CREATE_INVITE]: {
    label: "Create invite",
    about: "Make invite links that let people join, even when the server isn't in Browse.",
    channel: "Make invite links that open this channel.",
  },
  [P.MANAGE_EMOJI]: { label: "Manage emoji", about: "Add, rename and delete the server's own emoji." },
  [P.CONNECT]: { label: "Connect", about: "Join voice channels.", channel: "Join this voice channel." },
  [P.SPEAK]: {
    label: "Speak",
    about: "Talk in voice channels. Without it they can join and listen.",
    channel: "Talk in this voice channel. Without it they can join and listen.",
  },
  [P.VIDEO]: {
    label: "Video",
    about: "Turn their camera on and share their screen in voice channels.",
    channel: "Turn their camera on and share their screen in this voice channel.",
  },
  [P.RECORD]: {
    label: "Record",
    about:
      "Record voice channels on their device or on the server, and download and delete the server's recordings. People using fuwa see it and hear a beep; nothing can stop someone recording their speakers with other software.",
    channel:
      "Record this voice channel on their device or on the server, and download and delete its recordings. People using fuwa see it and hear a beep; nothing can stop someone recording their speakers with other software.",
  },
  [P.MUTE_MEMBERS]: {
    label: "Mute members",
    about: "Mute or deafen people ranked below them in voice channels, for everyone.",
    channel: "Mute or deafen people ranked below them in this voice channel.",
  },
  [P.MOVE_MEMBERS]: {
    label: "Move members",
    about: "Disconnect people ranked below them from voice channels.",
    channel: "Disconnect people ranked below them from this voice channel.",
  },
  [P.MANAGE_WEBHOOKS]: {
    label: "Manage webhooks",
    about: "Make, change and delete webhooks, and see their addresses, which let other apps post in any channel.",
  },
};

export const permissionInfo = (p: P): PermissionInfo => (p === P.UNSPECIFIED ? { label: "", about: "" } : PERMISSIONS[p]);
export const permissionLabel = (p: P) => permissionInfo(p).label;

/** Permissions as the settings pages group them. */
export const PERMISSION_GROUPS: { title: string; permissions: P[] }[] = [
  {
    title: "Server",
    permissions: [P.VIEW_CHANNELS, P.MANAGE_CHANNELS, P.MANAGE_ROLES, P.MANAGE_EMOJI, P.MANAGE_WEBHOOKS, P.MANAGE_SERVER, P.VIEW_AUDIT_LOG],
  },
  {
    title: "Membership",
    permissions: [P.CREATE_INVITE, P.CHANGE_NICKNAME, P.MANAGE_NICKNAMES, P.KICK_MEMBERS, P.BAN_MEMBERS, P.TIME_OUT_MEMBERS],
  },
  {
    title: "Text channels",
    permissions: [P.SEND_MESSAGES, P.EMBED_LINKS, P.ATTACH_FILES, P.MENTION_EVERYONE, P.MANAGE_MESSAGES],
  },
  { title: "Voice channels", permissions: [P.CONNECT, P.SPEAK, P.VIDEO, P.RECORD, P.MUTE_MEMBERS, P.MOVE_MEMBERS] },
  { title: "Advanced", permissions: [P.ADMINISTRATOR] },
];

/** The channel permissions, grouped for a channel's settings. */
export const CHANNEL_GROUPS: { title: string; permissions: P[] }[] = [
  { title: "General", permissions: [P.VIEW_CHANNELS, P.MANAGE_CHANNELS, P.MANAGE_ROLES, P.CREATE_INVITE] },
  {
    title: "Text",
    permissions: [P.SEND_MESSAGES, P.EMBED_LINKS, P.ATTACH_FILES, P.MENTION_EVERYONE, P.MANAGE_MESSAGES],
  },
  { title: "Voice", permissions: [P.CONNECT, P.SPEAK, P.VIDEO, P.RECORD, P.MUTE_MEMBERS, P.MOVE_MEMBERS] },
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
  roleIds: readonly string[],
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
      if (!isRole(o) || !roleIds.includes(o.targetId)) continue;
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
  const held = roles.filter((r) => roleIds.includes(r.id));
  const everyone = roles.find((r) => r.id === everyoneId);
  const base = held.reduce((bits, r) => bits | fromList(r.permissions), everyone ? fromList(everyone.permissions) : 0);
  const rank = owner ? Number.MAX_SAFE_INTEGER : Math.max(0, ...held.map((r) => r.position));
  const unbound = owner || !!(base & bit(P.ADMINISTRATOR));
  const byId = new Map(channels.map((c) => [c.id, c]));
  const visible = new Map<string, Bits>();
  for (const c of channels) {
    const bits = unbound ? ALL : channelBits(base, everyoneId, userId, roleIds, c, byId);
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
export const rolesOf = (roles: readonly Role[], member: Pick<Member, "roleIds"> | undefined) =>
  member ? roles.filter((r) => member.roleIds.includes(r.id)) : [];

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
