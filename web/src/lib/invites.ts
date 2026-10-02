import type { Invite } from "@/gen/fuwa/v1/types_pb";
import { instanceKey, normalizeUrl } from "@/fuwa/saved";
import { toDate } from "@/lib/format";

/** How long a new invite lasts, in seconds; 0 is forever. */
export const EXPIRE_AFTER = [
  { value: 1800, label: "30 minutes" },
  { value: 3600, label: "1 hour" },
  { value: 6 * 3600, label: "6 hours" },
  { value: 12 * 3600, label: "12 hours" },
  { value: 86_400, label: "1 day" },
  { value: 7 * 86_400, label: "7 days" },
  { value: 0, label: "Never" },
] as const;

/** How many people one invite lets in; 0 is anyone with the link. */
export const MAX_USES = [
  { value: 0, label: "No limit" },
  { value: 1, label: "1 use" },
  { value: 5, label: "5 uses" },
  { value: 10, label: "10 uses" },
  { value: 25, label: "25 uses" },
  { value: 50, label: "50 uses" },
  { value: 100, label: "100 uses" },
] as const;

/** What a fresh invite starts as, like Discord's: a week, for anyone. */
export const DEFAULT_INVITE = { maxAgeSeconds: 7 * 86_400, maxUses: 0 };

/** The smallest account ages a server can ask for before letting someone in. */
export const ACCOUNT_AGES = [
  { value: 0, label: "Any age" },
  { value: 600, label: "10 minutes" },
  { value: 3600, label: "1 hour" },
  { value: 86_400, label: "1 day" },
  { value: 7 * 86_400, label: "1 week" },
] as const;

/**
 * An invite's address: on the instance itself, so it opens in any browser,
 * with or without fuwa, and leads straight to the invite page.
 */
export const inviteLink = (base: string, code: string) => `${base.replace(/\/+$/, "")}/invite/${code}`;

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

/** Time until something, in its largest unit, rounded: "45 minutes", "6 hours", "7 days". */
export function timeLeft(ms: number) {
  const minutes = Math.max(1, Math.round(ms / 60_000));
  const [n, unit] = minutes >= 1440 ? [Math.round(minutes / 1440), "day"] : minutes >= 60 ? [Math.round(minutes / 60), "hour"] : [minutes, "minute"];
  return `${n} ${unit}${n === 1 ? "" : "s"}`;
}
