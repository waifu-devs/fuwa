/**
 * What people type in the search bar, read like Discord's: words, plus
 * filters written `key:value` anywhere among them (`from:mika in:general
 * has:link before:2026-10-01 cake`). Words go to the instance as they are;
 * filters are turned into ids and times by the search bar, which knows the
 * server's members and channels. Recent searches are kept on this device
 * only, never on an instance.
 */

export type FilterKey = "from" | "in" | "mentions" | "has" | "before" | "after" | "during";

export const FILTER_KEYS: { key: FilterKey; hint: string; example: string }[] = [
  { key: "from", hint: "a member", example: "from: user" },
  { key: "in", hint: "a channel", example: "in: channel" },
  { key: "mentions", hint: "a member", example: "mentions: user" },
  { key: "has", hint: "link, embed, file, picture, video, sound", example: "has: link" },
  { key: "before", hint: "a date", example: "before: 2026-10-01" },
  { key: "during", hint: "a date", example: "during: today" },
  { key: "after", hint: "a date", example: "after: yesterday" },
];

/** What `has:` takes, and the words that mean each. */
export const HAS_VALUES: { value: string; label: string; aliases: string[] }[] = [
  { value: "link", label: "link", aliases: ["link", "links", "url"] },
  { value: "embed", label: "embed", aliases: ["embed", "embeds"] },
  { value: "file", label: "file", aliases: ["file", "files", "attachment"] },
  { value: "picture", label: "picture", aliases: ["picture", "pictures", "image", "images", "photo"] },
  { value: "video", label: "video", aliases: ["video", "videos"] },
  { value: "sound", label: "sound", aliases: ["sound", "sounds", "audio"] },
  { value: "everyone", label: "@everyone", aliases: ["everyone", "@everyone", "here", "@here"] },
];

export type Filter = { key: FilterKey; value: string; start: number; end: number };
export type Parsed = { text: string; filters: Filter[] };

const KEYS = new Set<string>(FILTER_KEYS.map((f) => f.key));

/** Splits what was typed into words and filters. A filter with nothing after its colon is just a word for now. */
export function parseQuery(input: string): Parsed {
  const filters: Filter[] = [];
  const words: string[] = [];
  for (const match of input.matchAll(/\S+/g)) {
    const token = match[0];
    const colon = token.indexOf(":");
    const key = colon > 0 ? token.slice(0, colon).toLowerCase() : "";
    const value = colon > 0 ? token.slice(colon + 1) : "";
    if (KEYS.has(key) && value) {
      filters.push({ key: key as FilterKey, value, start: match.index, end: match.index + token.length });
    } else {
      words.push(token);
    }
  }
  // A space at the end means the last word is finished (the instance then matches only that word, not longer ones).
  const text = words.join(" ") + (words.length && /\s$/.test(input) ? " " : "");
  return { text, filters };
}

/** The word the caret is in (or right after), for suggestions. */
export function tokenAt(input: string, caret: number): { start: number; end: number; text: string } {
  let start = caret;
  while (start > 0 && !/\s/.test(input[start - 1]!)) start--;
  let end = caret;
  while (end < input.length && !/\s/.test(input[end]!)) end++;
  return { start, end, text: input.slice(start, end) };
}

/** Puts `replacement` where the word at the caret is, with a space after; answers the new text and caret. */
export function replaceToken(input: string, caret: number, replacement: string): { input: string; caret: number } {
  const { start, end } = tokenAt(input, caret);
  const after = input.slice(end).replace(/^\s*/, "");
  const next = `${input.slice(0, start)}${replacement} ${after}`;
  return { input: next, caret: start + replacement.length + 1 };
}

/** What `has:` value a word means, if any. */
export function hasValue(word: string): string | null {
  const w = word.toLowerCase();
  return HAS_VALUES.find((h) => h.aliases.includes(w))?.value ?? null;
}

/** A day typed as 2026-10-04, today or yesterday: its local midnight. */
export function parseDay(value: string, now: Date): Date | null {
  const v = value.toLowerCase();
  const midnight = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
  if (v === "today") return midnight(now);
  if (v === "yesterday") return new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1);
  const m = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(v);
  if (!m) return null;
  const [y, mo, d] = [Number(m[1]), Number(m[2]) - 1, Number(m[3])];
  const day = new Date(y, mo, d);
  return day.getFullYear() === y && day.getMonth() === mo && day.getDate() === d ? day : null;
}

const nextDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1);

/**
 * The time a search covers, from its date filters: `before:` a day is up to
 * its start, `after:` a day from the next one's (as Discord reads them), and
 * `during:` that whole day. Several narrow it down. `bad` is a filter whose
 * date couldn't be read.
 */
export function timeRange(filters: Filter[], now: Date): { after?: Date; before?: Date; bad?: Filter } {
  let after: Date | undefined;
  let before: Date | undefined;
  const later = (a: Date | undefined, b: Date) => (a && a > b ? a : b);
  const earlier = (a: Date | undefined, b: Date) => (a && a < b ? a : b);
  for (const f of filters) {
    if (f.key !== "before" && f.key !== "after" && f.key !== "during") continue;
    const day = parseDay(f.value, now);
    if (!day) return { bad: f };
    if (f.key === "before") before = earlier(before, day);
    if (f.key === "after") after = later(after, nextDay(day));
    if (f.key === "during") {
      after = later(after, day);
      before = earlier(before, nextDay(day));
    }
  }
  return { after, before };
}

// ── Recent searches, on this device ─────────────────────────────────────────

const RECENT_KEY = "fuwa:search:recent:v1";
const MAX_RECENT = 6;

type Storage = { getItem(key: string): string | null; setItem(key: string, value: string): void };

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function readAll(from: Storage | null): Record<string, string[]> {
  try {
    const raw = from?.getItem(RECENT_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return parsed && typeof parsed === "object" ? (parsed as Record<string, string[]>) : {};
  } catch {
    return {};
  }
}

/** The searches made lately in one server, newest first. */
export function recentSearches(place: string, from: Storage | null = storage()): string[] {
  const list = readAll(from)[place];
  return Array.isArray(list) ? list.filter((q) => typeof q === "string").slice(0, MAX_RECENT) : [];
}

/** Remembers a search on this device (or, with `forget`, takes it off the list). */
export function rememberSearch(place: string, query: string, forget = false, to: Storage | null = storage()) {
  const q = query.trim();
  if (!q || !to) return;
  const all = readAll(to);
  const list = (all[place] ?? []).filter((x) => x !== q);
  all[place] = forget ? list : [q, ...list].slice(0, MAX_RECENT);
  try {
    to.setItem(RECENT_KEY, JSON.stringify(all));
  } catch {
    // Storage full or blocked: recent searches just aren't kept.
  }
}

/** Forgets every recent search in one server. */
export function clearRecentSearches(place: string, to: Storage | null = storage()) {
  if (!to) return;
  const all = readAll(to);
  delete all[place];
  try {
    to.setItem(RECENT_KEY, JSON.stringify(all));
  } catch {
    // Nothing kept, nothing to clear.
  }
}

// ── Highlights ──────────────────────────────────────────────────────────────

/** Marks put around matches in a message's text before it's drawn; the Markdown plugin turns them into highlights. */
export const MARK_OPEN = "";
export const MARK_CLOSE = "";

/** `text` with marks around each range (UTF-16 offsets, in order, not overlapping). */
export function markRanges(text: string, ranges: { start: number; end: number }[]): string {
  let out = "";
  let at = 0;
  for (const r of ranges) {
    if (r.start < at || r.end > text.length || r.end <= r.start) continue;
    out += text.slice(at, r.start) + MARK_OPEN + text.slice(r.start, r.end) + MARK_CLOSE;
    at = r.end;
  }
  return out + text.slice(at);
}
