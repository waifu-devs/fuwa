import { timestampDate } from "@bufbuild/protobuf/wkt";
import { useEffect, useState } from "react";
import { NotificationLevel, type Message, type NotificationSettings, type User } from "@/gen/fuwa/v1/types_pb";
import { notificationKey, useFuwa, type InstanceState } from "@/fuwa/store";
import { i18n, type I18n, type Key } from "@/i18n/i18n";
import { formatTime, mentions } from "@/lib/format";
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
 * Whether a message pings you: by your @username or your id (`<@id>`, as
 * apps write it), or through @everyone, @here (unless you hid those) or one
 * of your roles. The server works out who may ping everyone, which roles a
 * message reached and which ids name members.
 */
export function pingsMe(
  inst: InstanceState,
  serverId: string,
  message: Pick<Message, "authorId" | "content" | "mentionsEveryone" | "mentionRoleIds"> & Partial<Pick<Message, "mentionUserIds">>,
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
  message: Pick<Message, "authorId" | "content" | "mentionsEveryone" | "mentionRoleIds"> & Partial<Pick<Message, "mentionUserIds">>,
  suppressEveryone: boolean,
) {
  if (!me || message.authorId === me.id) return false;
  if (mentions(message.content, me.username) || message.mentionUserIds?.includes(me.id)) return true;
  if (message.mentionsEveryone && !suppressEveryone) return true;
  if (!message.mentionRoleIds.length) return false;
  const mine = new Set(myRoleIds);
  return message.mentionRoleIds.some((id) => mine.has(id));
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

/** The levels, named in the app's language (common.notify.*): `label` for menus, `short` for chips. */
export const LEVELS = [
  { value: NotificationLevel.ALL, label: "common.notify.all", short: "common.notify.allShort" },
  { value: NotificationLevel.MENTIONS, label: "common.notify.mentions", short: "common.notify.mentionsShort" },
  { value: NotificationLevel.NOTHING, label: "common.notify.nothing", short: "common.notify.nothingShort" },
] as const satisfies readonly { value: NotificationLevel; label: Key; short: Key }[];

/** How long a mute can last, like Discord's menu. `null` is until you turn it back on. */
export const MUTE_FOR = [
  { id: "15m", ms: 15 * 60_000 },
  { id: "1h", ms: 60 * 60_000 },
  { id: "3h", ms: 3 * 60 * 60_000 },
  { id: "8h", ms: 8 * 60 * 60_000 },
  { id: "24h", ms: 24 * 60 * 60_000 },
  { id: "forever", ms: null },
] as const;

/** A mute length as the menu says it: "For 15 minutes", "For 1 hour", "Until I turn it back on". */
export function muteForLabel(t: I18n["t"], m: (typeof MUTE_FOR)[number]) {
  if (m.ms === null) return t("common.notify.forever");
  const minutes = m.ms / 60_000;
  return minutes < 60 ? t("common.notify.forMinutes", { count: minutes }) : t("common.notify.forHours", { count: minutes / 60 });
}

/** When a timed mute runs out: "4:30 PM", or "Tue 9:00 AM" on another day; "" when it lasts until turned off. */
export function mutedUntil(n: NotificationSettings | undefined, now = Date.now()) {
  if (!n?.mutedUntil) return "";
  const until = timestampDate(n.mutedUntil);
  const time = formatTime(until);
  return until.toDateString() === new Date(now).toDateString() ? time : `${i18n().date(until, { weekday: "short" })} ${time}`;
}

/** Beside "Unmute": "until 4:30 PM", or nothing when it lasts until turned off. */
export function mutedHint(t: I18n["t"], n: NotificationSettings | undefined, now = Date.now()) {
  const time = mutedUntil(n, now);
  return time ? t("common.notify.until", { time }) : "";
}

/** "Muted until 4:30 PM", "Muted until Tue 9:00 AM", or "Muted". */
export function mutedLabel(t: I18n["t"], n: NotificationSettings | undefined, now = Date.now()) {
  const time = mutedUntil(n, now);
  return time ? t("common.notify.mutedUntil", { time }) : t("common.notify.muted");
}
