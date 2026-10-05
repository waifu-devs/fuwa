import type { Invite } from "@/gen/fuwa/v1/types_pb";
import { getInstance } from "@/fuwa/hooks";
import { instanceKey, normalizeUrl } from "@/fuwa/saved";
import type { Key } from "@/i18n/i18n";
import { formatDuration, type Lang, toDate } from "@/lib/format";

/** How long a new invite lasts, in seconds; 0 is forever. Labels are catalog keys. */
export const EXPIRE_AFTER: readonly { value: number; label: Key }[] = [
  { value: 1800, label: "workspace.invite.expire.minutes30" },
  { value: 3600, label: "workspace.invite.expire.hour" },
  { value: 6 * 3600, label: "workspace.invite.expire.hours6" },
  { value: 12 * 3600, label: "workspace.invite.expire.hours12" },
  { value: 86_400, label: "workspace.invite.expire.day" },
  { value: 7 * 86_400, label: "workspace.invite.expire.days7" },
  { value: 0, label: "workspace.invite.expire.never" },
];

/** How many people one invite lets in; 0 is anyone with the link. */
export const MAX_USES = [0, 1, 5, 10, 25, 50, 100] as const;

/** What a fresh invite starts as, like Discord's: a week, for anyone. */
export const DEFAULT_INVITE = { maxAgeSeconds: 7 * 86_400, maxUses: 0 };

/** The smallest account ages a server can ask for before letting someone in; labels are catalog keys. */
export const ACCOUNT_AGES: readonly { value: number; label: Key }[] = [
  { value: 0, label: "serversettings.access.age.any" },
  { value: 600, label: "serversettings.access.age.tenMinutes" },
  { value: 3600, label: "serversettings.access.age.hour" },
  { value: 86_400, label: "serversettings.access.age.day" },
  { value: 7 * 86_400, label: "serversettings.access.age.week" },
];

/**
 * An invite's address: on the instance itself, so it opens in any browser,
 * with or without fuwa, and leads straight to the invite page.
 */
export const inviteLink = (base: string, code: string) => `${base.replace(/\/+$/, "")}/invite/${code}`;

/** An instance's own address, as links to it should read. */
export const publicBase = (key: string) => {
  const inst = getInstance(key);
  return inst?.node?.publicUrl || inst?.url || "";
};

/** When an invite stops working, or null if it doesn't. */
export const expiresAt = (invite: Invite): Date | null => (invite.expiresAt ? toDate(invite.expiresAt) : null);

/** Whether an invite still lets people in. */
export const works = (invite: Invite, now = Date.now()) =>
  (invite.maxUses === 0 || invite.uses < invite.maxUses) && (expiresAt(invite)?.getTime() ?? Infinity) > now;

const CODE = /^[A-Za-z0-9]{1,32}$/;

/**
 * Reads what someone pasted as an invite: a link from any instance, or just
 * a code, which then belongs to the instance they're looking at.
 */
export function parseInvite(text: string, here: string): { instance: string; code: string } | null {
  const pasted = text.trim().replace(/[?#].*$/, "").replace(/\/+$/, "");
  if (CODE.test(pasted)) return { instance: here, code: pasted };
  const at = pasted.lastIndexOf("/invite/");
  if (at <= 0) return null;
  const code = pasted.slice(at + "/invite/".length);
  if (!CODE.test(code)) return null;
  try {
    return { instance: instanceKey(normalizeUrl(pasted.slice(0, at))), code };
  } catch {
    return null;
  }
}

/** Time until something, in its largest unit, rounded, in the app's language: "45 minutes", "6 hours", "7 days". */
export function timeLeft(lang: Lang, ms: number) {
  const minutes = Math.max(1, Math.round(ms / 60_000));
  const seconds = minutes >= 1440 ? Math.round(minutes / 1440) * 86_400 : minutes >= 60 ? Math.round(minutes / 60) * 3600 : minutes * 60;
  return formatDuration(lang, seconds);
}
