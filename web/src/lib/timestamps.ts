// Timestamps in messages, written the way Discord writes them: `<t:SECONDS>`
// or `<t:SECONDS:STYLE>`, where SECONDS is a Unix time and STYLE one of the
// letters below. Everyone reads them in their own time zone and language, and
// the same text means the same thing in Discord, so tokens can be pasted
// either way. Only the browser's Intl APIs format them; nothing about the
// reader's time zone ever leaves the device.

export type TimestampStyle = "t" | "T" | "d" | "D" | "f" | "F" | "R";

/** Every style, in the order the picker lists them. */
export const STYLES: { style: TimestampStyle; name: string }[] = [
  { style: "t", name: "Short time" },
  { style: "T", name: "Long time" },
  { style: "d", name: "Short date" },
  { style: "D", name: "Long date" },
  { style: "f", name: "Date and time" },
  { style: "F", name: "Day, date and time" },
  { style: "R", name: "Relative" },
];

/** What `<t:SECONDS>` with no style shows, as in Discord. */
export const DEFAULT_STYLE: TimestampStyle = "f";

/** A token anywhere in text. Group 1 is the seconds, group 2 the style. */
export const TOKEN = /<t:(-?\d{1,14})(?::([tTdDfFR]))?>/g;

/** The furthest a JavaScript date reaches either way, in seconds. */
const LIMIT = 8_640_000_000_000;

/** A token's time and style, or null when it isn't one (or is out of range). */
export function parseToken(seconds: string, style?: string): { ms: number; style: TimestampStyle } | null {
  const n = Number(seconds);
  if (!Number.isSafeInteger(n) || Math.abs(n) > LIMIT) return null;
  if (style !== undefined && !STYLES.some((s) => s.style === style)) return null;
  return { ms: n * 1000, style: (style as TimestampStyle | undefined) ?? DEFAULT_STYLE };
}

/** The token for a moment, ready to send. The default style is written out as Discord's picker leaves it: bare. */
export function toToken(date: Date, style: TimestampStyle): string {
  const seconds = Math.floor(date.getTime() / 1000);
  return style === DEFAULT_STYLE ? `<t:${seconds}>` : `<t:${seconds}:${style}>`;
}

const OPTIONS: Record<Exclude<TimestampStyle, "R">, Intl.DateTimeFormatOptions> = {
  t: { hour: "numeric", minute: "2-digit" },
  T: { hour: "numeric", minute: "2-digit", second: "2-digit" },
  d: { year: "numeric", month: "2-digit", day: "2-digit" },
  D: { year: "numeric", month: "long", day: "numeric" },
  f: { year: "numeric", month: "long", day: "numeric", hour: "numeric", minute: "2-digit" },
  F: { weekday: "long", year: "numeric", month: "long", day: "numeric", hour: "numeric", minute: "2-digit" },
};

// Building a formatter is the slow part, and every message reuses the same few.
const dateFormats = new Map<string, Intl.DateTimeFormat>();
const relativeFormats = new Map<string, Intl.RelativeTimeFormat>();

function dateFormat(style: Exclude<TimestampStyle, "R">, locale?: string, timeZone?: string) {
  const key = `${style}|${locale ?? ""}|${timeZone ?? ""}`;
  let f = dateFormats.get(key);
  if (!f) dateFormats.set(key, (f = new Intl.DateTimeFormat(locale, { ...OPTIONS[style], timeZone })));
  return f;
}

function relativeFormat(locale?: string) {
  const key = locale ?? "";
  let f = relativeFormats.get(key);
  if (!f) relativeFormats.set(key, (f = new Intl.RelativeTimeFormat(locale, { numeric: "auto" })));
  return f;
}

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const MONTH = 30 * DAY;
const YEAR = 365 * DAY;

/**
 * The unit a gap reads in, with the same cut-offs Discord uses: seconds up
 * to 45 of them, minutes up to 45, hours up to 22, days up to 26, months up
 * to 11, then years.
 */
function unitFor(gap: number): [Intl.RelativeTimeFormatUnit, number] {
  const abs = Math.abs(gap);
  if (abs < 45 * SECOND) return ["second", SECOND];
  if (abs < 45 * MINUTE) return ["minute", MINUTE];
  if (abs < 22 * HOUR) return ["hour", HOUR];
  if (abs < 26 * DAY) return ["day", DAY];
  if (abs < 11 * MONTH) return ["month", MONTH];
  return ["year", YEAR];
}

/** "in 3 hours", "2 days ago": how far a moment is from now. */
export function formatRelative(ms: number, now: number, locale?: string): string {
  const gap = ms - now;
  const [unit, size] = unitFor(gap);
  // At least one of the unit, so the seconds read "in 5 seconds", not "now", until the moment comes.
  const amount = Math.round(gap / size);
  return relativeFormat(locale).format(amount === 0 && unit !== "second" ? Math.sign(gap) : amount, unit);
}

/** A timestamp as its style shows it, in the reader's time zone (or `timeZone`, for tests). */
export function formatTimestamp(ms: number, style: TimestampStyle, now: number, locale?: string, timeZone?: string): string {
  if (style === "R") return formatRelative(ms, now, locale);
  return dateFormat(style, locale, timeZone).format(ms);
}

/** The whole date, for the card that opens on hover or a long press. */
export const formatFull = (ms: number, locale?: string, timeZone?: string) => dateFormat("F", locale, timeZone).format(ms);

/**
 * How long until a relative timestamp could read differently: a second
 * while it counts seconds, else a minute (hours and days read in whole
 * units, so a minute is soon enough).
 */
export const refreshEvery = (ms: number, now: number) => (unitFor(ms - now)[0] === "second" ? SECOND : MINUTE);

// The remark plugin: tokens in text (outside code and links) become `fuwa-time` elements.

type MdNode = { type: string; value?: string; children?: MdNode[]; data?: Record<string, unknown> };

function split(value: string): MdNode[] | null {
  if (!value.includes("<t:")) return null;
  const out: MdNode[] = [];
  let last = 0;
  for (const m of value.matchAll(TOKEN)) {
    if (!parseToken(m[1]!, m[2])) continue;
    const at = m.index ?? 0;
    if (at > last) out.push({ type: "text", value: value.slice(last, at) });
    out.push({
      type: "timestamp",
      data: { hName: "fuwa-time", hProperties: { dataTime: m[1], dataStyle: m[2] ?? "" } },
      children: [{ type: "text", value: m[0] }],
    });
    last = at + m[0].length;
  }
  if (!out.length) return null;
  if (last < value.length) out.push({ type: "text", value: value.slice(last) });
  return out;
}

/** Leaves code and links alone. */
const SKIP = new Set(["code", "inlineCode", "link", "linkReference", "html", "timestamp"]);

function walk(node: MdNode) {
  if (!node.children) return;
  const next: MdNode[] = [];
  for (const child of node.children) {
    if (child.type === "text" && child.value) {
      const parts = split(child.value);
      if (parts) {
        next.push(...parts);
        continue;
      }
    } else if (!SKIP.has(child.type)) walk(child);
    next.push(child);
  }
  node.children = next;
}

/** The remark plugin that turns `<t:...>` tokens into `fuwa-time` elements. */
export function remarkTimestamps() {
  return (tree: MdNode) => walk(tree);
}
