import { Effect } from "effect";
import { create } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { Code } from "@connectrpc/connect";
import type { AccountFilter, InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import type { UpdateProfileRequest } from "@/gen/fuwa/v1/auth_pb";
import type { ChannelPlacement } from "@/gen/fuwa/v1/channel_pb";
import type { MediaPurpose } from "@/gen/fuwa/v1/media_pb";
import type { AuditAction } from "@/gen/fuwa/v1/server_pb";
import {
  ApplicationStatus,
  ChannelType,
  JoinFormSchema,
  type AnnouncementTone,
  type Application,
  type Channel,
  type Member,
  type NotificationLevel,
  type Permission,
  type PermissionOverwrite,
  type Role,
  type NotificationSettings,
  type Server,
  type ServerLimits,
} from "@/gen/fuwa/v1/types_pb";
import { saveApplied, type Applied } from "@/lib/applied";
import { canReturnTo, newSecret, savePending, sha256Hex, type PendingSignIn } from "@/lib/linked";
import { accessOf, canSee, sortRoles } from "@/lib/permissions";
import { makeApi } from "./client";
import { call, FuwaError, toFuwaError } from "./errors";
import { normalizeUrl } from "./saved";
import { addInstance, engine, follow, removeInstance } from "./sync";
import {
  addServer,
  notificationKey,
  removeServer,
  sortChannels,
  sortMembers,
  store,
  updateInstance,
  upsertMessage,
  withUpdatedUser,
  withUsers,
  type PendingMessage,
} from "./store";

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

/**
 * Signs in with a password. Accounts with two-step sign-in answer with a
 * ticket instead, which `verifyTwoFactor` turns into a session with a code.
 */
export const signIn = (url: string, username: string, password: string) =>
  Effect.gen(function* () {
    const res = yield* call((signal) => makeApi(url, () => null).auth.signIn({ username, password }, { signal }));
    if (res.twoFactorTicket) return { ticket: res.twoFactorTicket } as const;
    return { key: addInstance(url, res.token) } as const;
  });

export const verifyTwoFactor = (url: string, ticket: string, code: string) =>
  Effect.gen(function* () {
    const res = yield* call((signal) => makeApi(url, () => null).auth.verifyTwoFactor({ ticket, code }, { signal }));
    return addInstance(url, res.token);
  });

export const signUp = (url: string, username: string, password: string, displayName: string) =>
  Effect.gen(function* () {
    const res = yield* call((signal) =>
      makeApi(url, () => null).auth.signUp({ username, password, displayName }, { signal }),
    );
    return addInstance(url, res.token);
  });

/**
 * Starts signing in with waifu.dev: remembers the sign-in in this tab, then
 * sends the browser to waifu.dev. It comes back to the callback page, which
 * finishes with `finishLinkedSignIn`.
 */
export const startLinkedSignIn = (url: string, next: string | null) =>
  Effect.gen(function* () {
    if (!canReturnTo(window.location.origin)) {
      return yield* Effect.fail(toFuwaError(new Error("signing in with waifu.dev needs this page on an https address")));
    }
    const secret = newSecret();
    const secretHash = yield* Effect.promise(() => sha256Hex(secret));
    const res = yield* call((signal) =>
      makeApi(url, () => null).auth.startLinkedSignIn({ returnOrigin: window.location.origin, secretHash }, { signal }),
    );
    if (!savePending(res.state, { url, secret, next, startedAt: Date.now() })) {
      return yield* Effect.fail(toFuwaError(new Error("this browser won't keep the sign-in while you visit waifu.dev")));
    }
    window.location.assign(res.authorizeUrl);
    return true;
  });

/** Finishes a sign-in with waifu.dev that this tab started. */
export const finishLinkedSignIn = (pending: PendingSignIn, state: string, code: string) =>
  Effect.gen(function* () {
    const res = yield* call((signal) =>
      makeApi(pending.url, () => null).auth.finishLinkedSignIn({ state, code, secret: pending.secret }, { signal }),
    );
    return { key: addInstance(pending.url, res.token), user: res.user, created: res.created };
  });

/** Which app a sign-in that came back to this instance belongs to. */
export const linkedSignInOrigin = (url: string, state: string) =>
  Effect.gen(function* () {
    const res = yield* call((signal) => makeApi(url, () => null).auth.getLinkedSignIn({ state }, { signal }));
    return res.returnOrigin;
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

export type ProfilePatch = Partial<
  Pick<UpdateProfileRequest, "displayName" | "avatarUrl" | "pronouns" | "bio" | "bannerUrl" | "accentColor" | "status">
> & { statusExpiresAt?: Date | null };

/** Changes your profile; only the fields given change. */
export const updateProfile = (key: string, patch: ProfilePatch) =>
  Effect.gen(function* () {
    const { statusExpiresAt, ...rest } = patch;
    const request = { ...rest, statusExpiresAt: statusExpiresAt ? timestampFromDate(statusExpiresAt) : undefined };
    const { user, profile } = yield* call((signal) => api(key).auth.updateProfile(request, { signal }));
    updateInstance(key, (i) => {
      const next = user ? withUpdatedUser(i, user) : i;
      return profile && user ? { ...next, profiles: { ...next.profiles, [user.id]: profile } } : next;
    });
    return true;
  });

// ───────────────────────── Pictures ─────────────────────────

/** How an upload failed, from the status of the PUT and the words the server sent back. */
const PUT_FAILURES: Record<number, Code> = {
  400: Code.InvalidArgument,
  404: Code.FailedPrecondition,
  408: Code.DeadlineExceeded,
  413: Code.ResourceExhausted,
  415: Code.InvalidArgument,
};

/**
 * Uploads a picture (an avatar, a banner or a server icon) and resolves to
 * its link, ready to set. `progress` hears how much has gone, from 0 to 1.
 * The bytes go to the instance's own address, whatever name it gave the link.
 */
export const uploadPicture = (key: string, purpose: MediaPurpose, file: Blob, progress?: (sent: number) => void) =>
  Effect.gen(function* () {
    const { uploadUrl, media } = yield* call((signal) =>
      api(key).media.createUpload({ purpose, contentType: file.type, size: BigInt(file.size) }, { signal }),
    );
    const token = uploadUrl.slice(uploadUrl.lastIndexOf("/") + 1);
    const target = `${engine(key).url.replace(/\/+$/, "")}/media/upload/${token}`;
    yield* Effect.async<void, FuwaError>((resume) => {
      const xhr = new XMLHttpRequest();
      xhr.open("PUT", target);
      xhr.upload.onprogress = (e) => e.lengthComputable && progress?.(e.loaded / e.total);
      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          progress?.(1);
          return resume(Effect.void);
        }
        const message = xhr.responseText.trim() || "the upload didn't go through";
        resume(Effect.fail(new FuwaError({ code: PUT_FAILURES[xhr.status] ?? Code.Unavailable, message })));
      };
      xhr.onerror = () => resume(Effect.fail(new FuwaError({ code: Code.Unavailable, message: "can't reach this server right now" })));
      xhr.send(file);
      return Effect.sync(() => xhr.abort());
    });
    return media!.url;
  });

/** Someone's full profile: pronouns, bio, banner. Kept so it shows at once next time. */
export const loadProfile = (key: string, userId: string) =>
  Effect.gen(function* () {
    const { profile } = yield* call((signal) => api(key).auth.getProfile({ userId }, { signal }));
    if (profile) updateInstance(key, (i) => ({ ...i, profiles: { ...i.profiles, [userId]: profile } }));
    return profile!;
  });

/** Changes a standalone account's password. The server signs out every other session. */
export const changePassword = (key: string, currentPassword: string, newPassword: string) =>
  call((signal) => api(key).auth.changePassword({ currentPassword, newPassword }, { signal })).pipe(Effect.as(true));

// ───────────────────────── Your account ─────────────────────────

export const listSessions = (key: string) =>
  call((signal) => api(key).account.listSessions({}, { signal })).pipe(Effect.map((r) => r.sessions));

export const revokeSession = (key: string, sessionId: string) =>
  call((signal) => api(key).account.revokeSession({ sessionId }, { signal })).pipe(Effect.as(true));

export const revokeOtherSessions = (key: string) =>
  call((signal) => api(key).account.revokeOtherSessions({}, { signal })).pipe(Effect.map((r) => r.revoked));

export const getTwoFactor = (key: string) => call((signal) => api(key).account.getTwoFactor({}, { signal }));

export const setUpTwoFactor = (key: string, password: string) =>
  call((signal) => api(key).account.setUpTwoFactor({ password }, { signal }));

export const enableTwoFactor = (key: string, code: string) =>
  call((signal) => api(key).account.enableTwoFactor({ code }, { signal })).pipe(Effect.map((r) => r.backupCodes));

export const disableTwoFactor = (key: string, password: string, code: string) =>
  call((signal) => api(key).account.disableTwoFactor({ password, code }, { signal })).pipe(Effect.as(true));

export const regenerateBackupCodes = (key: string, password: string) =>
  call((signal) => api(key).account.regenerateBackupCodes({ password }, { signal })).pipe(
    Effect.map((r) => r.backupCodes),
  );

export type NotificationPatch = Partial<Pick<NotificationSettings, "level" | "suppressEveryone">> & {
  /** Muted until then; `null` mutes until turned back on, `false` unmutes. */
  mutedUntil?: Date | null | false;
};

/** Changes how a server (or one of its channels) notifies you, on every device. */
export const updateNotifications = (key: string, serverId: string, channelId: string, patch: NotificationPatch) =>
  Effect.gen(function* () {
    const paths: string[] = [];
    if (patch.level !== undefined) paths.push("level");
    if (patch.suppressEveryone !== undefined) paths.push("suppress_everyone");
    if (patch.mutedUntil !== undefined) paths.push("muted");
    const settings = {
      serverId,
      channelId,
      level: patch.level,
      suppressEveryone: patch.suppressEveryone,
      muted: patch.mutedUntil !== undefined && patch.mutedUntil !== false,
      mutedUntil: patch.mutedUntil ? timestampFromDate(patch.mutedUntil) : undefined,
    };
    const res = yield* call((signal) =>
      api(key).account.updateNotificationSettings({ settings, updateMask: { paths } }, { signal }),
    );
    const saved = res.settings;
    updateInstance(key, (i) => {
      const { [notificationKey(serverId, channelId)]: _, ...rest } = i.notifications;
      const says = saved && (saved.level || saved.muted || saved.suppressEveryone);
      return { ...i, notifications: says ? { ...rest, [notificationKey(serverId, channelId)]: saved } : rest };
    });
    return saved!;
  });

/** Reads your notification settings again, for changes made on another device. */
export const refreshNotifications = (key: string) =>
  Effect.gen(function* () {
    const { settings } = yield* call((signal) => api(key).account.getNotificationSettings({}, { signal }));
    const notifications = Object.fromEntries(settings.map((n) => [notificationKey(n.serverId, n.channelId), n]));
    updateInstance(key, (i) => ({ ...i, notifications }));
  });

/**
 * Picks up what changed without an event while you were away: notification
 * settings from another device, and the instance's details and announcement.
 */
export function refreshOnFocus() {
  let last = Date.now();
  window.addEventListener("focus", () => {
    if (Date.now() - last < 60_000) return;
    last = Date.now();
    for (const [key, i] of Object.entries(store.get().instances)) {
      if (i.me && i.connection === "live") {
        run(refreshNotifications(key)).catch(() => {});
        run(refreshNode(key)).catch(() => {});
      }
    }
  });
}

/** Everything the instance keeps about you, as one JSON file. `progress` hears the bytes so far. */
export const exportData = (key: string, progress: (bytes: number) => void) =>
  Effect.tryPromise({
    try: async (signal) => {
      const parts: Uint8Array<ArrayBuffer>[] = [];
      let bytes = 0;
      for await (const res of api(key).account.exportData({}, { signal })) {
        parts.push(new Uint8Array(res.chunk));
        bytes += res.chunk.length;
        progress(bytes);
      }
      return new Blob(parts, { type: "application/json" });
    },
    catch: toFuwaError,
  });

/** Deletes your account on an instance, then forgets the instance here. */
export const deleteAccount = (key: string, confirm: { password?: string; code?: string; username?: string }) =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).account.deleteAccount(confirm, { signal }));
    removeInstance(key);
    return true;
  });

/** Puts a member's new state in the store, ahead of its event. */
function storeMember(key: string, serverId: string, member: Member | undefined) {
  if (!member?.user) return;
  const id = member.user.id;
  updateInstance(key, (i) => ({
    ...i,
    members: { ...i.members, [serverId]: sortMembers([...(i.members[serverId] ?? []).filter((m) => m.user?.id !== id), member]) },
  }));
}

/** Sets a nickname in a server: yours (no `userId`), or someone's ranked below you. */
export const setNickname = (key: string, serverId: string, nickname: string, userId = "") =>
  Effect.gen(function* () {
    const { member } = yield* call((signal) =>
      api(key).servers.updateMember({ serverId, userId, nickname }, { signal }),
    );
    storeMember(key, serverId, member);
    return member!;
  });

// ───────────────────────── Moderation (owners and admins) ─────────────────────────

// ───────────────────────── Roles ─────────────────────────

/** Puts roles in the store, ahead of their events. */
function storeRoles(key: string, serverId: string, changed: Role[]) {
  const ids = new Set(changed.map((r) => r.id));
  updateInstance(key, (i) => ({
    ...i,
    roles: { ...i.roles, [serverId]: sortRoles([...(i.roles[serverId] ?? []).filter((r) => !ids.has(r.id)), ...changed]) },
  }));
}

/** Gives someone a role ranked below your highest one. */
export const giveRole = (key: string, serverId: string, userId: string, roleId: string) =>
  Effect.gen(function* () {
    const { member } = yield* call((signal) => api(key).roles.addMemberRole({ serverId, userId, roleId }, { signal }));
    storeMember(key, serverId, member);
    return member!;
  });

export const takeRole = (key: string, serverId: string, userId: string, roleId: string) =>
  Effect.gen(function* () {
    const { member } = yield* call((signal) => api(key).roles.removeMemberRole({ serverId, userId, roleId }, { signal }));
    storeMember(key, serverId, member);
    return member!;
  });

export type RoleDraft = {
  name: string;
  color?: number;
  permissions: Permission[];
  hoist: boolean;
  mentionable: boolean;
};

/** A new role, right above @everyone. */
export const createRole = (key: string, serverId: string, draft: RoleDraft) =>
  Effect.gen(function* () {
    const { role } = yield* call((signal) => api(key).roles.createRole({ serverId, ...draft }, { signal }));
    // Every other role moved up one; their events say so too.
    updateInstance(key, (i) => ({
      ...i,
      roles: {
        ...i.roles,
        [serverId]: (i.roles[serverId] ?? []).map((r) => (r.id !== serverId ? { ...r, position: r.position + 1 } : r)),
      },
    }));
    storeRoles(key, serverId, [role!]);
    return role!;
  });

/** Changes what's given; `color: null` clears it. @everyone takes only `permissions`. */
export const updateRole = (key: string, serverId: string, roleId: string, patch: Partial<Omit<RoleDraft, "color">> & { color?: number | null }) =>
  Effect.gen(function* () {
    const { color, permissions, ...rest } = patch;
    const { role } = yield* call((signal) =>
      api(key).roles.updateRole(
        {
          serverId,
          roleId,
          ...rest,
          clearColor: color === null,
          color: color ?? undefined,
          permissions: permissions ? { permissions } : undefined,
        },
        { signal },
      ),
    );
    storeRoles(key, serverId, [role!]);
    return role!;
  });

export const deleteRole = (key: string, serverId: string, roleId: string) =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).roles.deleteRole({ serverId, roleId }, { signal }));
    return true;
  });

/** Every role but @everyone, highest first. */
export const reorderRoles = (key: string, serverId: string, roleIds: string[]) =>
  Effect.gen(function* () {
    const { roles } = yield* call((signal) => api(key).roles.reorderRoles({ serverId, roleIds }, { signal }));
    storeRoles(key, serverId, roles);
    return roles;
  });

/** Who can do what in one channel, all its overwrites at once. */
export const setChannelPermissions = (
  key: string,
  serverId: string,
  channelId: string,
  overwrites: Omit<PermissionOverwrite, "$typeName" | "$unknown">[],
) =>
  Effect.gen(function* () {
    const { channel } = yield* call((signal) =>
      api(key).channels.setChannelPermissions({ serverId, channelId, overwrites }, { signal }),
    );
    // Unless it just hid the channel from you; then its event takes it away.
    const i = store.get().instances[key];
    const me = i?.members[serverId]?.find((m) => m.user?.id === i.me?.id);
    const server = i?.servers.find((s) => s.id === serverId);
    if (channel && i && me && server) {
      const others = (i.channels[serverId] ?? []).filter((c) => c.id !== channel.id);
      const access = accessOf(serverId, server.ownerId, i.roles[serverId] ?? [], [...others, channel], i.me!.id, me.roleIds);
      if (canSee(access, channel.id)) storeChannels(key, serverId, [channel]);
    }
    return channel!;
  });

/** Times someone out for `seconds`; 0 ends it. */
export const timeOutMember = (key: string, serverId: string, userId: string, seconds: number, reason = "") =>
  Effect.gen(function* () {
    const { member } = yield* call((signal) =>
      api(key).servers.timeOutMember({ serverId, userId, seconds: BigInt(seconds), reason }, { signal }),
    );
    storeMember(key, serverId, member);
    return member!;
  });

const dropMember = (key: string, serverId: string, userId: string) =>
  updateInstance(key, (i) => {
    const list = i.members[serverId] ?? [];
    if (!list.some((m) => m.user?.id === userId)) return i;
    return {
      ...i,
      members: { ...i.members, [serverId]: list.filter((m) => m.user?.id !== userId) },
      servers: i.servers.map((s) => (s.id === serverId ? { ...s, memberCount: s.memberCount - 1n } : s)),
    };
  });

export const kickMember = (key: string, serverId: string, userId: string, reason = "") =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).servers.kickMember({ serverId, userId, reason }, { signal }));
    dropMember(key, serverId, userId);
    return true;
  });

/** Bans someone, taking what they sent in the last `deleteSeconds` with them. Returns how many messages went. */
export const banMember = (key: string, serverId: string, userId: string, reason: string, deleteSeconds: number) =>
  Effect.gen(function* () {
    const res = yield* call((signal) =>
      api(key).servers.banMember({ serverId, userId, reason, deleteMessageSeconds: BigInt(deleteSeconds) }, { signal }),
    );
    dropMember(key, serverId, userId);
    return Number(res.deletedMessages);
  });

export const unbanMember = (key: string, serverId: string, userId: string) =>
  call((signal) => api(key).servers.unbanMember({ serverId, userId }, { signal })).pipe(Effect.as(true));

export const listBans = (key: string, serverId: string) => call((signal) => api(key).servers.listBans({ serverId }, { signal }));

export type AuditFilter = { actorId?: string; action?: AuditAction; beforeId?: string };

export const listAuditLog = (key: string, serverId: string, filter: AuditFilter = {}) =>
  call((signal) => api(key).servers.listAuditLog({ serverId, limit: 50, ...filter }, { signal }));

/** Hands the server to another member; you stay on as an admin. */
export const transferOwnership = (key: string, serverId: string, userId: string) =>
  Effect.gen(function* () {
    const { server } = yield* call((signal) => api(key).servers.transferOwnership({ serverId, userId }, { signal }));
    if (server) updateInstance(key, (i) => addServer(i, server));
    return server!;
  });

// ───────────────────────── Servers ─────────────────────────

const joined = (key: string, server: Server | undefined) =>
  Effect.gen(function* () {
    if (!server) return;
    updateInstance(key, (i) => addServer(i, server));
    yield* follow(key, server.id);
  });

export const createServer = (key: string, name: string, description: string, discoverable: boolean, iconUrl = "") =>
  Effect.gen(function* () {
    const { server } = yield* call((signal) =>
      api(key).servers.createServer({ name, description, discoverable, iconUrl }, { signal }),
    );
    yield* joined(key, server);
    return server!;
  });

export const discover = (key: string) =>
  call((signal) => api(key).servers.discoverServers({}, { signal })).pipe(Effect.map((r) => r.servers));

/** Joins a server from Browse, or with one of its invites. */
export const joinServer = (key: string, serverId: string, inviteCode = "") =>
  Effect.gen(function* () {
    const { server } = yield* call((signal) => api(key).servers.joinServer({ serverId, inviteCode }, { signal }));
    yield* joined(key, server);
    return server!;
  });

// ───────────────────────── Rules and applications ─────────────────────────

/** A server's rules and questions; `inviteCode` for one you aren't in that's out of Browse. */
export const getJoinForm = (key: string, serverId: string, inviteCode = "") =>
  call((signal) => api(key).join.getJoinForm({ serverId, inviteCode }, { signal })).pipe(
    Effect.map((r) => r.form ?? create(JoinFormSchema)),
  );

export type QuestionDraft = { prompt: string; paragraph: boolean; required: boolean };

export const setJoinForm = (key: string, serverId: string, rules: string[], questions: QuestionDraft[]) =>
  Effect.gen(function* () {
    const { form } = yield* call((signal) =>
      api(key).join.setJoinForm({ serverId, form: { rules, questions } }, { signal }),
    );
    updateInstance(key, (i) => {
      const server = i.servers.find((s) => s.id === serverId);
      return server ? addServer(i, { ...server, hasRules: rules.length > 0 }) : i;
    });
    return form ?? create(JoinFormSchema);
  });

/** Agrees to a server's rules, so you can talk there. */
export const agreeToRules = (key: string, serverId: string) =>
  Effect.gen(function* () {
    const { member } = yield* call((signal) => api(key).join.agreeToRules({ serverId }, { signal }));
    storeMember(key, serverId, member);
    return true;
  });

/** Changes which servers you're waiting on, here and in this browser's memory. */
function setApplied(key: string, serverId: string, applied: Applied | null) {
  updateInstance(key, (i) => {
    const { [serverId]: _, ...rest } = i.applied;
    const next = applied ? { ...rest, [serverId]: applied } : rest;
    saveApplied(key, next);
    return { ...i, applied: next };
  });
}

/** Asks to join a server that lets people in by hand, answering its questions as shown. */
export const applyToJoin = (
  key: string,
  server: Server,
  inviteCode: string,
  answers: { question: string; answer: string }[],
) =>
  Effect.gen(function* () {
    const { application } = yield* call((signal) =>
      api(key).join.applyToJoin({ serverId: server.id, inviteCode, answers }, { signal }),
    );
    setApplied(key, server.id, { server, status: ApplicationStatus.PENDING, reason: "", appliedAt: Date.now(), inviteCode });
    return application!;
  });

export const withdrawApplication = (key: string, serverId: string) =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).join.withdrawApplication({ serverId }, { signal })).pipe(
      // Already gone on the server: forgetting it here is all that's left.
      Effect.catchIf((e) => e.code === Code.NotFound, () => Effect.void),
    );
    setApplied(key, serverId, null);
    return true;
  });

/** Lets go of a turned-down application in the rail. */
export const dismissApplied = (key: string, serverId: string) => setApplied(key, serverId, null);

/**
 * Asks how each waiting application went: the ones that were let in become
 * servers, turned-down ones say why, and ones that are gone go. Returns the
 * servers that let you in and the ones that turned you down, to say so.
 */
export const checkApplied = (key: string) =>
  Effect.gen(function* () {
    const waiting = Object.values(store.get().instances[key]?.applied ?? {}).filter((a) => a.status === ApplicationStatus.PENDING);
    const letIn: Server[] = [];
    const turnedDown: Server[] = [];
    for (const a of waiting) {
      const res = yield* call((signal) => api(key).join.getApplication({ serverId: a.server.id }, { signal })).pipe(
        Effect.map((r) => ({ found: r })),
        Effect.catchIf((e) => e.code === Code.NotFound, () => Effect.succeed({ found: null })),
        Effect.catchAll(() => Effect.succeed(undefined)),
      );
      if (!res) continue; // couldn't reach it; ask again later
      const { found } = res;
      if (found?.member && found.server) {
        setApplied(key, a.server.id, null);
        yield* joined(key, found.server);
        letIn.push(found.server);
      } else if (!found?.application) {
        setApplied(key, a.server.id, null);
      } else if (found.application.status === ApplicationStatus.REJECTED) {
        setApplied(key, a.server.id, { ...a, status: ApplicationStatus.REJECTED, reason: found.application.reason });
        turnedDown.push(a.server);
      }
    }
    return { letIn, turnedDown };
  });

/** The applications waiting in a server, for people who review them. */
export const listApplications = (key: string, serverId: string) =>
  Effect.gen(function* () {
    const { applications } = yield* call((signal) => api(key).join.listApplications({ serverId }, { signal }));
    updateInstance(key, (i) => ({
      ...i,
      applications: { ...i.applications, [serverId]: applications },
      users: withUsers(i.users, applications.map((a) => a.user)),
    }));
    return applications;
  });

export const reviewApplication = (key: string, serverId: string, application: Application, approve: boolean, reason = "") =>
  Effect.gen(function* () {
    const userId = application.user!.id;
    const { member } = yield* call((signal) =>
      api(key).join.reviewApplication({ serverId, userId, approve, reason }, { signal }),
    );
    updateInstance(key, (i) => {
      const list = i.applications[serverId];
      const applications = list ? { ...i.applications, [serverId]: list.filter((a) => a.user?.id !== userId) } : i.applications;
      return { ...i, applications };
    });
    if (member) storeMember(key, serverId, member);
    return member ?? null;
  });

// ───────────────────────── Invites ─────────────────────────

/** Where an invite leads, on an instance you may not be signed in to yet. */
export const lookUpInvite = (address: string, code: string) =>
  Effect.gen(function* () {
    const { url, node } = yield* probe(address);
    const found = yield* call((signal) => makeApi(url, () => null).invites.getInvite({ code }, { signal }));
    return { url, node, invite: found.invite!, server: found.server!, channelName: found.channelName, inviter: found.inviter };
  });

export type InviteOptions = { channelId?: string; maxUses: number; maxAgeSeconds: number };

export const createInvite = (key: string, serverId: string, { channelId = "", maxUses, maxAgeSeconds }: InviteOptions) =>
  call((signal) => api(key).invites.createInvite({ serverId, channelId, maxUses, maxAgeSeconds }, { signal })).pipe(
    Effect.map((r) => r.invite!),
  );

export const listInvites = (key: string, serverId: string) =>
  call((signal) => api(key).invites.listInvites({ serverId }, { signal }));

export const deleteInvite = (key: string, serverId: string, code: string) =>
  call((signal) => api(key).invites.deleteInvite({ serverId, code }, { signal })).pipe(Effect.as(true));

export const leaveServer = (key: string, serverId: string) =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).servers.leaveServer({ serverId }, { signal }));
    updateInstance(key, (i) => removeServer(i, serverId));
    return true;
  });

export const deleteServer = (key: string, serverId: string) =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).servers.deleteServer({ serverId }, { signal }));
    updateInstance(key, (i) => removeServer(i, serverId));
    return true;
  });

export const updateServer = (
  key: string,
  serverId: string,
  patch: {
    name?: string;
    description?: string;
    /** Empty for no icon. */
    iconUrl?: string;
    discoverable?: boolean;
    defaultNotifications?: NotificationLevel;
    /** Empty for no join messages. */
    systemChannelId?: string;
    minAccountAgeSeconds?: number;
    applications?: boolean;
    linkedOnly?: boolean;
  },
) =>
  Effect.gen(function* () {
    const { server } = yield* call((signal) => api(key).servers.updateServer({ serverId, ...patch }, { signal }));
    if (server) updateInstance(key, (i) => addServer(i, server));
    return true;
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

/** Accounts on the instance, newest first, with what admins need to look after them. */
export const listAccounts = (
  key: string,
  query: { query?: string; filter?: AccountFilter; beforeId?: string; limit?: number } = {},
) => call((signal) => api(key).admin.listAccounts(query, { signal }));

/** Makes an account an admin or not, and turns it off (with a reason) or back on. */
export const updateAccount = (
  key: string,
  accountId: string,
  change: { admin?: boolean; disabled?: boolean; reason?: string },
) =>
  call((signal) => api(key).admin.updateAccount({ accountId, ...change }, { signal })).pipe(
    Effect.map((r) => r.account!),
  );

/** Gives an account a new random password, shown this once. Signs it out everywhere. */
export const resetAccountPassword = (key: string, accountId: string, turnOffTwoFactor: boolean) =>
  call((signal) => api(key).admin.resetAccountPassword({ accountId, turnOffTwoFactor }, { signal })).pipe(
    Effect.map((r) => r.password),
  );

/** Every server on the instance, with its owner, usage and caps. */
export const listInstanceServers = (key: string) =>
  call((signal) => api(key).admin.listInstanceServers({}, { signal })).pipe(Effect.map((r) => r.servers));

/**
 * A server's whole database as one SQLite file. `progress` hears the bytes so
 * far and the total once the first piece says it.
 */
export const exportServer = (key: string, serverId: string, progress: (bytes: number, total: number) => void) =>
  Effect.tryPromise({
    try: async (signal) => {
      const parts: Uint8Array<ArrayBuffer>[] = [];
      let bytes = 0;
      let total = 0;
      let filename = "server.db";
      for await (const res of api(key).admin.exportServer({ serverId }, { signal })) {
        if (res.filename) filename = res.filename;
        if (res.size) total = Number(res.size);
        parts.push(new Uint8Array(res.chunk));
        bytes += res.chunk.length;
        progress(bytes, total);
      }
      return { blob: new Blob(parts, { type: "application/vnd.sqlite3" }), filename };
    },
    catch: toFuwaError,
  });

/** Puts up the banner every client of this instance shows, or takes it down with empty text. */
export const setAnnouncement = (
  key: string,
  announcement: { text: string; tone: AnnouncementTone; endsAt?: Date },
) =>
  Effect.gen(function* () {
    const res = yield* call((signal) =>
      api(key).admin.setAnnouncement(
        {
          announcement: {
            text: announcement.text,
            tone: announcement.tone,
            endsAt: announcement.endsAt ? timestampFromDate(announcement.endsAt) : undefined,
          },
        },
        { signal },
      ),
    );
    updateInstance(key, (i) => (i.node ? { ...i, node: { ...i.node, announcement: res.announcement } } : i));
    return res.announcement;
  });

/** Reads the instance's public details again: its name, sign-up options and announcement. */
export const refreshNode = (key: string) =>
  Effect.gen(function* () {
    const { node } = yield* call((signal) => api(key).node.getNode({}, { signal }));
    if (node) updateInstance(key, (i) => ({ ...i, node }));
  });

// ───────────────────────── Channels ─────────────────────────

export const createChannel = (key: string, serverId: string, name: string, type: ChannelType, parentId = "") =>
  Effect.gen(function* () {
    const { channel } = yield* call((signal) =>
      api(key).channels.createChannel({ serverId, name, type, parentId }, { signal }),
    );
    // So opening it right away doesn't race its event.
    if (channel) storeChannels(key, serverId, [channel]);
    return channel!;
  });

/** Puts channels in the store, ahead of their events. */
function storeChannels(key: string, serverId: string, changed: Channel[]) {
  const ids = new Set(changed.map((c) => c.id));
  updateInstance(key, (i) => ({
    ...i,
    channels: { ...i.channels, [serverId]: sortChannels([...(i.channels[serverId] ?? []).filter((c) => !ids.has(c.id)), ...changed]) },
  }));
}

export const updateChannel = (
  key: string,
  serverId: string,
  channelId: string,
  patch: { name?: string; topic?: string; parentId?: string; slowmodeSeconds?: number },
) =>
  Effect.gen(function* () {
    const { channel } = yield* call((signal) => api(key).channels.updateChannel({ serverId, channelId, ...patch }, { signal }));
    if (channel) storeChannels(key, serverId, [channel]);
    return channel!;
  });

/** Every channel of a server in its new order, each in its category (or none). */
export const reorderChannels = (key: string, serverId: string, channels: Pick<ChannelPlacement, "channelId" | "parentId">[]) =>
  Effect.gen(function* () {
    const res = yield* call((signal) => api(key).channels.reorderChannels({ serverId, channels }, { signal }));
    storeChannels(key, serverId, res.channels);
    return res.channels;
  });

export const deleteChannel = (key: string, serverId: string, channelId: string) =>
  Effect.gen(function* () {
    yield* call((signal) => api(key).channels.deleteChannel({ serverId, channelId }, { signal }));
    updateInstance(key, (i) => ({
      ...i,
      channels: { ...i.channels, [serverId]: (i.channels[serverId] ?? []).filter((c) => c.id !== channelId) },
    }));
    return true;
  });

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

/** Clears the unread counts of every channel in a server. Returns how many channels had some. */
export function markServerRead(key: string, serverId: string): number {
  let cleared = 0;
  updateInstance(key, (i) => {
    const unread = { ...i.unread };
    for (const channel of i.channels[serverId] ?? []) {
      if (!unread[channel.id]) continue;
      delete unread[channel.id];
      cleared++;
    }
    return cleared ? { ...i, unread } : i;
  });
  return cleared;
}

export { ChannelType };
