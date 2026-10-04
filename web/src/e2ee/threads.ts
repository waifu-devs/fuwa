/**
 * Threads inside a secure channel, worked out on the device. The server
 * can't read the channel, so it can't sum threads up the way it does for
 * ordinary channels (docs/threads.md): a reply carries the record of the
 * message it's under inside the encryption, and every device sorts the lines
 * it opened into the channel and its threads itself. Nothing here asks the
 * server for anything: what isn't on this device stays unknown, so the
 * server never learns which records make up a thread.
 */

import type { Item, Note } from "./vault";

/** What a thread is, as this device can tell from the lines it has. */
export type SecureThread = {
  /** The record of the message it's under. */
  parent: number;
  replies: number;
  /** When the last reply was sent (Unix ms), and its record; 0 with none. */
  lastAt: number;
  lastSeq: number;
  /** The five latest people to reply, the latest first. */
  participants: string[];
  locked: boolean;
};

export type Organized = {
  /** What the channel itself shows: everything but replies kept to their threads, and lock changes. */
  channel: Item[];
  /** Threads by the record of their message. */
  threads: Map<number, SecureThread>;
  /** Each thread's replies and lock changes, oldest first. */
  inThread: Map<number, Item[]>;
};

const PARTICIPANTS = 5;

/** A line that can have a thread under it: text someone wrote, still there, not itself a reply. */
export const canHaveThread = (i: Item | undefined): i is Item => !!i && i.kind === "text" && !i.deleted && !i.thread;

/**
 * The thread a line replies in, or 0 if it shows as an ordinary line: no
 * thread named, or the line it names is here and can't have one (a device
 * line, another reply, or a later record). A thread whose message isn't on
 * this device still counts: its message may come later, or never did.
 */
export function threadOf(i: Item, bySeq: ReadonlyMap<number, Item>): number {
  const parent = i.thread ?? 0;
  if (parent <= 0 || parent >= i.seq || (i.kind !== "text" && i.kind !== "thread")) return 0;
  const p = bySeq.get(parent);
  if (p && (p.kind !== "text" || p.thread)) return 0;
  return parent;
}

/**
 * Sorts a secure channel's lines into the channel and its threads. A lock
 * change counts only from someone `moderates` says has Manage Messages in
 * the channel (the device's own view of its permissions); the latest wins.
 */
export function organize(items: readonly Item[], moderates: (userId: string) => boolean): Organized {
  const bySeq = new Map(items.map((i) => [i.seq, i]));
  const channel: Item[] = [];
  const threads = new Map<number, SecureThread>();
  const inThread = new Map<number, Item[]>();
  const thread = (parent: number) => {
    let t = threads.get(parent);
    if (!t)
      threads.set(
        parent,
        (t = {
          parent,
          replies: 0,
          lastAt: 0,
          lastSeq: 0,
          participants: [],
          locked: false,
        }),
      );
    return t;
  };
  for (const i of items) {
    const parent = threadOf(i, bySeq);
    if (i.kind === "thread") {
      // A lock on a message that's gone, or that this device never had, makes no thread of its own.
      const p = bySeq.get(parent);
      if (!parent || !moderates(i.senderId) || !p || p.deleted) continue;
      const t = thread(parent);
      t.locked = i.content === "locked";
      inThread.set(parent, [...(inThread.get(parent) ?? []), i]);
      continue;
    }
    if (!parent) {
      channel.push(i);
      continue;
    }
    // A thread goes with its message.
    if (bySeq.get(parent)?.deleted) continue;
    inThread.set(parent, [...(inThread.get(parent) ?? []), i]);
    if (i.inChannel) channel.push(i);
    if (i.deleted) continue;
    const t = thread(parent);
    t.replies++;
    t.lastAt = i.at;
    t.lastSeq = i.seq;
    t.participants = [i.senderId, ...t.participants.filter((p) => p !== i.senderId)].slice(0, PARTICIPANTS);
  }
  for (const [parent, t] of threads) if (!t.replies && (!t.locked || !canHaveThread(bySeq.get(parent)))) threads.delete(parent);
  return { channel, threads, inThread };
}

/** Whether you follow a thread: as you set it by hand, or else if you wrote its message or replied in it. */
export function following(note: Pick<Note, "follows"> | undefined, parent: number, items: readonly Item[], me: string): boolean {
  const set = note?.follows?.[parent];
  if (set !== undefined) return set;
  return items.some((i) => i.senderId === me && i.kind === "text" && (i.seq === parent || i.thread === parent));
}

/** Replies in a thread you haven't seen yet: others', still there, after the last one you saw. */
export function unreadIn(note: Pick<Note, "threadRead"> | undefined, parent: number, replies: readonly Item[], me: string): number {
  const seen = note?.threadRead?.[parent] ?? 0;
  return replies.filter((i) => i.kind === "text" && !i.deleted && i.senderId !== me && i.seq > seen).length;
}

/** Whether nobody has replied for longer than the server keeps threads open (0 hours: never archived). */
export const archived = (t: SecureThread | undefined, hours: number, now = Date.now()) =>
  !!t && hours > 0 && t.lastAt > 0 && now - t.lastAt > hours * 3_600_000;

/**
 * Replies to drop from this device because the message they're under was
 * deleted: the thread goes with its message. Nothing is deleted on the
 * server (a burst of deletes would tell it which records were replies); the
 * replies' ciphertext stays there, unreadable, and is never passed on.
 */
export function orphaned(items: Iterable<Item>, bySeq: ReadonlyMap<number, Item>): Item[] {
  const out: Item[] = [];
  for (const i of items) {
    if (i.kind !== "text" || i.deleted || !i.thread) continue;
    if (bySeq.get(i.thread)?.deleted) out.push({ ...i, deleted: true, content: "" });
  }
  return out;
}

/** Threads whose message or replies say `query`, newest reply first. */
export function search(org: Organized, bySeq: ReadonlyMap<number, Item>, query: string): SecureThread[] {
  const q = query.trim().toLowerCase();
  const all = [...org.threads.values()].sort((a, b) => b.lastSeq - a.lastSeq || b.parent - a.parent);
  if (!q) return all;
  const says = (i: Item | undefined) => !!i && !i.deleted && i.content.toLowerCase().includes(q);
  return all.filter((t) => says(bySeq.get(t.parent)) || (org.inThread.get(t.parent) ?? []).some(says));
}

/** What deleting a line needs: the server's delete, and this device's copy, read and written under its lock. */
export type Deleting = {
  remove: (seq: number) => Promise<void>;
  locked: (fn: () => Promise<void>) => Promise<void>;
  load: () => Promise<Item[]>;
  write: (items: Item[]) => Promise<void>;
};

/**
 * Deletes one line: a single delete on the server, for that line alone, then
 * this device's copy. In a secure channel the line's thread goes with it on
 * this device (see `orphaned`); its replies are never deleted on the server.
 */
export async function deleteLine(io: Deleting, seq: number, secure: boolean) {
  await io.remove(seq);
  await forgetLine(io, seq, secure);
}

/** Marks a line deleted on this device (someone deleted it), and in a secure channel drops its thread here too. Asks the server nothing. */
export async function forgetLine(io: Omit<Deleting, "remove">, seq: number, secure: boolean) {
  await io.locked(async () => {
    const all = await io.load();
    const before = all.find((i) => i.seq === seq);
    if (!before || before.deleted) return;
    const gone = { ...before, deleted: true, content: "" };
    const bySeq = new Map(all.map((i) => [i.seq, i]));
    bySeq.set(seq, gone);
    await io.write([gone, ...(secure ? orphaned(all, bySeq) : [])]);
  });
}
