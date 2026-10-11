/**
 * How much of a conversation's history sits in memory (the store), as
 * opposed to the vault, which keeps all of it. A conversation nobody opened
 * keeps only its tail (enough for the list's preview) and its unread count;
 * one that's open keeps a window of its latest lines, reaching further back
 * as you scroll, and new records land in it one by one instead of the whole
 * history being read again. Pure, and only types come from elsewhere, so
 * node's test runner reads this file as is.
 */

import type { Item } from "./vault";

/** How many lines an opened direct message shows at first, and how many more each step back. */
export const OPEN_LINES = 200;
/** How many conversations keep their window after they're left (the one on screen not counted). */
export const KEEP_OPEN = 3;
/** How far back a tail looks for a message before giving up (devices lines and reactions between). */
export const TAIL_SCAN = 200;

const isMessage = (i: Pick<Item, "kind">) => i.kind === "text" || i.kind === "voice";

/** The lines a list's preview needs: from the last message on, without reactions. */
export function tailOf(lines: readonly Item[]): Item[] {
  for (let n = lines.length - 1; n >= 0; n--) {
    if (isMessage(lines[n]!)) return lines.slice(n).filter((i) => i.kind !== "reaction");
  }
  return lines.length ? [lines[lines.length - 1]!].filter((i) => i.kind !== "reaction") : [];
}

/** Whether two lists show the same lines, so the old one can stay (and nothing redraws). */
export function sameLines(a: readonly Item[] | undefined, b: readonly Item[]): boolean {
  if (!a || a.length !== b.length) return false;
  return a.every((x, n) => {
    const y = b[n]!;
    return x === y || (x.seq === y.seq && x.kind === y.kind && x.editedAt === y.editedAt && x.deleted === y.deleted && x.content === y.content);
  });
}

/** Where `seq` goes in lines sorted by seq: the first at or after it. Checks the end first, where new lines land. */
export function seqIndex(lines: readonly { seq: number }[], seq: number): number {
  const n = lines.length;
  if (!n || lines[n - 1]!.seq < seq) return n;
  let lo = 0;
  let hi = n - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (lines[mid]!.seq < seq) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** Puts a line in its place by seq, replacing the one already there. */
export function upsertLine<T extends { seq: number }>(lines: readonly T[], line: T): T[] {
  const at = seqIndex(lines, line.seq);
  if (at < lines.length && lines[at]!.seq === line.seq) {
    if (lines[at] === line) return lines as T[];
    const next = lines.slice();
    next[at] = line;
    return next;
  }
  if (at === lines.length) return [...lines, line];
  return [...lines.slice(0, at), line, ...lines.slice(at)];
}

/** An opened conversation's window: its lines (no reactions) and its reactions, from `from` on (0: from the start). */
export type Window = { lines: Item[]; reactions: Item[]; from: number };

/** Splits what the vault gave into lines and reactions. */
export function windowOf(all: readonly Item[], from: number): Window {
  const lines: Item[] = [];
  const reactions: Item[] = [];
  for (const i of all) (i.kind === "reaction" ? reactions : lines).push(i);
  return { lines, reactions, from };
}

/**
 * New or changed lines, put into a window. Lines older than the window are
 * left for when it reaches back to them. The same window comes back when
 * nothing in it changed, and each list only when it did.
 */
export function mergeWindow(w: Window, changed: Iterable<Item>): Window {
  let { lines, reactions } = w;
  for (const i of changed) {
    if (w.from > 0 && i.seq < w.from) continue;
    if (i.kind === "reaction") reactions = upsertLine(reactions, i);
    else lines = upsertLine(lines, i);
  }
  return lines === w.lines && reactions === w.reactions ? w : { lines, reactions, from: w.from };
}

/** Older lines, put in front of a window that now starts at `from`. */
export function extendWindow(w: Window, older: readonly Item[], from: number): Window {
  const add = windowOf(older.filter((i) => w.from === 0 || i.seq < w.from), from);
  return { lines: [...add.lines, ...w.lines], reactions: [...add.reactions, ...w.reactions], from };
}

/** Whether every line after `read` is in the window, so unread can be counted from it alone. */
export const coversUnread = (w: Window, read: number) => w.from === 0 || w.from <= read + 1;

/**
 * Recently opened conversations, the latest first: `id` comes to the front.
 * Returns the new order and the ones that fell off the end (more than `keep`
 * besides the one in front).
 */
export function touchRecent(recent: readonly string[], id: string, keep = KEEP_OPEN): { recent: string[]; dropped: string[] } {
  const next = [id, ...recent.filter((x) => x !== id)];
  return { recent: next.slice(0, keep + 1), dropped: next.slice(keep + 1) };
}
