import { timestampDate, type Timestamp } from "@bufbuild/protobuf/wkt";
import { AccountKind, type Member, type User } from "@/gen/fuwa/v1/types_pb";
import { i18n } from "@/i18n/i18n";
import { getPrefs } from "@/lib/prefs";

// Lengths of time and sizes; they take the language from the caller (useI18n).
export { ago, formatBytes, formatDuration, formatLeft, type Lang, roughly, shortDuration } from "@/lib/durations";

export const toDate = (ts: Timestamp | undefined) => (ts ? timestampDate(ts) : new Date(0));

/** The 12 or 24 hour clock setting; "auto" is the language's own. */
function hourCycle() {
  const setting = getPrefs().clock;
  return setting === "24h" ? "h23" : setting === "12h" ? "h12" : undefined;
}

const DAY = { weekday: "long", month: "long", day: "numeric" } as const;
const DAY_WITH_YEAR = { month: "long", day: "numeric", year: "numeric" } as const;

// Dates and times read in the app's language (i18n/).
export const formatTime = (d: Date) => i18n().date(d, { hour: "numeric", minute: "2-digit", hourCycle: hourCycle() });
export const formatFull = (d: Date) => i18n().date(d, { dateStyle: "full", timeStyle: "short", hourCycle: hourCycle() });

function startOfDay(d: Date) {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

export function sameDay(a: Date, b: Date) {
  return startOfDay(a) === startOfDay(b);
}

const daysAgo = (d: Date, now: Date) => Math.round((startOfDay(now) - startOfDay(d)) / 86_400_000);

/** "Today", "Yesterday", "Monday, June 3", or with the year when it's not this year. */
export function formatDay(d: Date, now = new Date()) {
  const { t, date } = i18n();
  const days = daysAgo(d, now);
  if (days === 0) return t("common.time.today");
  if (days === 1) return t("common.time.yesterday");
  return date(d, d.getFullYear() === now.getFullYear() ? DAY : DAY_WITH_YEAR);
}

/** "Today at 3:04 PM", "Yesterday at 9:12 AM", or the date and time. */
export function formatStamp(d: Date, now = new Date()) {
  const { t } = i18n();
  const days = daysAgo(d, now);
  const time = formatTime(d);
  if (days === 0) return t("common.time.todayAt", { time });
  if (days === 1) return t("common.time.yesterdayAt", { time });
  return t("common.time.dayTime", { day: formatDay(d, now), time });
}

/** Someone's name: their display name, else username, else "Someone" in the app's language. */
export const displayName = (user: User | undefined) => user?.displayName || user?.username || i18n().t("common.someone");
export const memberName = (member: Member | undefined) => member?.nickname || displayName(member?.user);
/** An agent: an account a program drives, not a person. */
export const isAgent = (user: User | undefined) => user?.kind === AccountKind.AGENT;

/** One or two letters for an icon: "Waifu Devs" → "WD", "fuwa" → "F". */
export function initials(name: string) {
  const words = name.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "?";
  const letters = words.length === 1 ? [words[0]!] : [words[0]!, words[words.length - 1]!];
  return letters.map((w) => Array.from(w)[0]!.toUpperCase()).join("");
}

/** A stable hue for something, from its id, so icons and names keep their colors. */
export function hueOf(id: string) {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return h % 360;
}

/** Whether a message mentions someone by @username. */
const mentionPatterns = new Map<string, RegExp>();
export function mentions(content: string, username: string) {
  // A chat checks every message against your name, so the pattern is made once per name.
  let pattern = mentionPatterns.get(username);
  if (!pattern) {
    pattern = new RegExp(`(^|[^\\w@])@${username.replace(/[.]/g, "\\.")}\\b`, "i");
    mentionPatterns.set(username, pattern);
  }
  return pattern.test(content);
}

/** Someone's custom status, unless it has run out. */
export function shownStatus(user: User | undefined, now = Date.now()) {
  if (!user?.status) return "";
  if (user.statusExpiresAt && toDate(user.statusExpiresAt).getTime() <= now) return "";
  return user.status;
}

/** A 0xRRGGBB profile color as CSS. */
export const colorCss = (color: number) => `#${color.toString(16).padStart(6, "0")}`;

/** When a member's time-out ends, if they're timed out now. */
export function timedOutUntil(member: Member | undefined, now = Date.now()): Date | null {
  if (!member?.timedOutUntil) return null;
  const until = toDate(member.timedOutUntil);
  return until.getTime() > now ? until : null;
}
