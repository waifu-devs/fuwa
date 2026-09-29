import { timestampDate, type Timestamp } from "@bufbuild/protobuf/wkt";
import type { Member, User } from "@/gen/fuwa/v1/types_pb";
import { getPrefs, type Clock } from "@/lib/prefs";

export const toDate = (ts: Timestamp | undefined) => (ts ? timestampDate(ts) : new Date(0));

const day = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric" });
const dayWithYear = new Intl.DateTimeFormat(undefined, { month: "long", day: "numeric", year: "numeric" });

/** Times follow the 12 or 24 hour clock setting; "auto" is the language's own. */
const clocks = new Map<Clock, { time: Intl.DateTimeFormat; full: Intl.DateTimeFormat }>();
function clock() {
  const setting = getPrefs().clock;
  let formats = clocks.get(setting);
  if (!formats) {
    const hourCycle = setting === "24h" ? "h23" : setting === "12h" ? "h12" : undefined;
    formats = {
      time: new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", hourCycle }),
      full: new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "short", hourCycle }),
    };
    clocks.set(setting, formats);
  }
  return formats;
}

export const formatTime = (d: Date) => clock().time.format(d);
export const formatFull = (d: Date) => clock().full.format(d);

function startOfDay(d: Date) {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

export function sameDay(a: Date, b: Date) {
  return startOfDay(a) === startOfDay(b);
}

/** "Today", "Yesterday", "Monday, June 3", or with the year when it's not this year. */
export function formatDay(d: Date, now = new Date()) {
  const days = Math.round((startOfDay(now) - startOfDay(d)) / 86_400_000);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  return d.getFullYear() === now.getFullYear() ? day.format(d) : dayWithYear.format(d);
}

/** "Today at 3:04 PM", "Yesterday at 9:12 AM", or the date and time. */
export function formatStamp(d: Date, now = new Date()) {
  const label = formatDay(d, now);
  return label === "Today" || label === "Yesterday" ? `${label} at ${formatTime(d)}` : `${label}, ${formatTime(d)}`;
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
export function mentions(content: string, username: string) {
  return new RegExp(`(^|[^\\w@])@${username.replace(/[.]/g, "\\.")}\\b`, "i").test(content);
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
