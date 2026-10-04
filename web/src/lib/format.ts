import { timestampDate, type Timestamp } from "@bufbuild/protobuf/wkt";
import { AccountKind, type Member, type User } from "@/gen/fuwa/v1/types_pb";
import { i18n } from "@/i18n/i18n";
import { getPrefs } from "@/lib/prefs";

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

export function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

export const displayName = (user: User | undefined) => user?.displayName || user?.username || "Someone";
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

const UNITS = [
  { seconds: 86_400, one: "day" },
  { seconds: 3_600, one: "hour" },
  { seconds: 60, one: "minute" },
  { seconds: 1, one: "second" },
] as const;

/** A length of time in its largest whole unit: "30 seconds", "5 minutes", "1 hour", "7 days". */
export function formatDuration(seconds: number) {
  const unit = UNITS.find((u) => seconds >= u.seconds && seconds % u.seconds === 0) ?? UNITS[3];
  const n = Math.round(seconds / unit.seconds);
  return `${n} ${unit.one}${n === 1 ? "" : "s"}`;
}

/** The same, clipped for a chip: "30s", "5m", "1h", "7d". */
export const shortDuration = (seconds: number) =>
  formatDuration(seconds).replace(/ (second|minute|hour|day)s?$/, (_, unit: string) => unit[0]!);

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/** How long something has been around, rounded down to its largest unit: "5 minutes", "3 days", "2 months". */
export function roughly(ms: number) {
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  if (minutes < 1) return "less than a minute";
  if (minutes < 60) return plural(minutes, "minute");
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return plural(hours, "hour");
  const days = Math.floor(hours / 24);
  if (days < 30) return plural(days, "day");
  if (days < 365) return plural(Math.floor(days / 30), "month");
  return plural(Math.floor(days / 365), "year");
}

/** "just now", "5 minutes ago", "3 days ago". */
export const ago = (date: Date, now = Date.now()) => (now - date.getTime() < 60_000 ? "just now" : `${roughly(now - date.getTime())} ago`);

/** Time left, as a countdown: "0:42", "12:05", "3h 20m", "2d 4h". */
export function formatLeft(ms: number) {
  const s = Math.max(0, Math.ceil(ms / 1000));
  if (s < 3600) return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  if (s < 86_400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  return `${Math.floor(s / 86_400)}d ${Math.floor((s % 86_400) / 3600)}h`;
}

/** When a member's time-out ends, if they're timed out now. */
export function timedOutUntil(member: Member | undefined, now = Date.now()): Date | null {
  if (!member?.timedOutUntil) return null;
  const until = toDate(member.timedOutUntil);
  return until.getTime() > now ? until : null;
}
