import { timestampDate } from "@bufbuild/protobuf/wkt";
import { useEffect, useState } from "react";
import { NotificationLevel, type Message, type NotificationSettings, type User } from "@/gen/fuwa/v1/types_pb";
import { notificationKey, useFuwa, type InstanceState } from "@/fuwa/store";
import { mentions } from "@/lib/format";
import type { Prefs } from "@/lib/prefs";

/**
 * How a message reaches you: your settings for its channel, else for its
 * server (both kept on the instance, so they follow you to every device),
 * else the server's default, else this device's own Notifications settings.
 */

export type Effective = {
  /** Unset when neither you nor the server's default says, so the device decides. */
  level: NotificationLevel;
  muted: boolean;
  suppressEveryone: boolean;
};

/** Whether settings mute, right now. */
export function isMuted(n: NotificationSettings | undefined, now = Date.now()) {
  if (!n?.muted) return false;
  return !n.mutedUntil || timestampDate(n.mutedUntil).getTime() > now;
}

export function effectiveNotifications(inst: InstanceState, serverId: string, channelId: string, now = Date.now()): Effective {
  const server = inst.notifications[notificationKey(serverId)];
  const channel = inst.notifications[notificationKey(serverId, channelId)];
  return {
    level:
      channel?.level ||
      server?.level ||
      inst.servers.find((s) => s.id === serverId)?.defaultNotifications ||
      NotificationLevel.UNSPECIFIED,
    muted: isMuted(server, now) || isMuted(channel, now),
    suppressEveryone: !!server?.suppressEveryone,
  };
}

/** Whether a message should reach you: sound, notification, or neither. */
export function shouldAlert(e: Effective, mention: boolean, prefs: Prefs): { sound: boolean; notify: boolean } {
  if (e.muted || e.level === NotificationLevel.NOTHING) return { sound: false, notify: false };
  if (e.level === NotificationLevel.MENTIONS) return { sound: mention, notify: mention };
  if (e.level === NotificationLevel.ALL) return { sound: true, notify: true };
  // Nothing set for this server: the device's settings decide.
  return { sound: true, notify: mention || prefs.notifyFor === "all" };
}

/**
 * Whether a message pings you: by your @username, or through @everyone,
 * @here (unless you hid those) or one of your roles. The server works out
 * who may ping everyone and which roles a message reached.
 */
export function pingsMe(
  inst: InstanceState,
  serverId: string,
  message: Pick<Message, "authorId" | "content" | "mentionsEveryone" | "mentionRoleIds">,
  suppressEveryone: boolean,
) {
  const me = inst.me;
  const mine = me ? inst.members[serverId]?.find((m) => m.user?.id === me.id)?.roleIds : undefined;
  return pingsUser(me ?? undefined, mine ?? [], message, suppressEveryone);
}

/** `pingsMe`, for callers that already know who you are and your roles (a message list checks every message). */
export function pingsUser(
  me: Pick<User, "id" | "username"> | undefined,
  myRoleIds: readonly string[],
  message: Pick<Message, "authorId" | "content" | "mentionsEveryone" | "mentionRoleIds">,
  suppressEveryone: boolean,
) {
  if (!me || message.authorId === me.id) return false;
  if (mentions(message.content, me.username)) return true;
  if (message.mentionsEveryone && !suppressEveryone) return true;
  if (!message.mentionRoleIds.length) return false;
  return message.mentionRoleIds.some((id) => myRoleIds.includes(id));
}

/** A clock that ticks every `ms`, so timed mutes run out on screen. */
export function useNow(ms = 30_000) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(id);
  }, [ms]);
  return now;
}

/** Your settings for a server, or one of its channels (none: all defaults). */
export function useNotificationSettings(key: string, serverId: string, channelId = ""): NotificationSettings | undefined {
  return useFuwa((s) => s.instances[key]?.notifications[notificationKey(serverId, channelId)]);
}

/** Whether a channel is muted, by itself or through its server. */
export function useMuted(key: string, serverId: string, channelId = ""): boolean {
  const now = useNow();
  const server = useNotificationSettings(key, serverId);
  const channel = useNotificationSettings(key, serverId, channelId);
  return isMuted(server, now) || isMuted(channel, now);
}

export const LEVELS = [
  { value: NotificationLevel.ALL, label: "All messages", short: "All" },
  { value: NotificationLevel.MENTIONS, label: "Only @mentions", short: "Mentions" },
  { value: NotificationLevel.NOTHING, label: "Nothing", short: "Nothing" },
] as const;

/** How long a mute can last, like Discord's menu. `null` is until you turn it back on. */
export const MUTE_FOR = [
  { label: "For 15 minutes", ms: 15 * 60_000 },
  { label: "For 1 hour", ms: 60 * 60_000 },
  { label: "For 3 hours", ms: 3 * 60 * 60_000 },
  { label: "For 8 hours", ms: 8 * 60 * 60_000 },
  { label: "For 24 hours", ms: 24 * 60 * 60_000 },
  { label: "Until I turn it back on", ms: null },
] as const;

/** "Muted until 4:30 PM", "Muted until Tue 9:00 AM", or "Muted". */
export function mutedLabel(n: NotificationSettings | undefined, now = Date.now()) {
  if (!n?.mutedUntil) return "Muted";
  const until = timestampDate(n.mutedUntil);
  const sameDay = until.toDateString() === new Date(now).toDateString();
  const time = until.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  return `Muted until ${sameDay ? time : `${until.toLocaleDateString(undefined, { weekday: "short" })} ${time}`}`;
}
