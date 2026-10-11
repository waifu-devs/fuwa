/**
 * Sorted lists the store keeps, changed one entry at a time instead of
 * sorted or scanned whole on every event, and how many messages it keeps
 * per channel. Pure, and only types come from elsewhere, so node's test
 * runner reads this file as is.
 */

import type { Member, Message } from "@/gen/fuwa/v1/types_pb";

// ───────────────────────── Messages ─────────────────────────

/** Where a message with `id` goes in a list sorted by id: the first at or after it. Checks the end first, where new ones land. */
export function messageIndex(items: readonly Pick<Message, "id">[], id: string): number {
  const n = items.length;
  if (!n || items[n - 1]!.id < id) return n;
  let lo = 0;
  let hi = n - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (items[mid]!.id < id) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** The message with `id` in a list sorted by id, if it's there. */
export function findMessage<T extends Pick<Message, "id">>(items: readonly T[], id: string): T | undefined {
  const at = messageIndex(items, id);
  return at < items.length && items[at]!.id === id ? items[at] : undefined;
}

/** A channel's (or thread's) messages as kept, for trimming. */
export type Loaded<M> = { items: M[]; hasMore: boolean; loading: boolean };

/** How many messages a channel keeps once it's out of sight: its latest page. */
export const KEEP_PAGE = 50;
/** How many messages the channel on screen keeps once you're back at its bottom. */
export const KEEP_AT_BOTTOM = 200;
/** How many channels and threads left lately keep their latest page; the rest let go of theirs. */
export const KEEP_CHANNELS = 8;

/** A channel cut back to its latest `keep` messages; it then has older ones to load again. */
export function trimmed<M>(loaded: Loaded<M>, keep: number): Loaded<M> {
  if (loaded.loading || loaded.items.length <= keep) return loaded;
  return { ...loaded, items: loaded.items.slice(-keep), hasMore: true };
}

/**
 * The places (channels, and threads by `threadKey`) opened lately, the
 * latest first, after `opened` came to the front. `null` leaves the order.
 */
export function touched(recent: readonly string[], opened: readonly (string | null | undefined)[]): string[] {
  const front = opened.filter((x): x is string => !!x);
  if (!front.length) return recent as string[];
  return [...front, ...recent.filter((x) => !front.includes(x))];
}

/**
 * Lets go of messages nobody's looking at. Places in `open` stay as they are;
 * the next `keepRecent` places opened lately keep only their latest page (and
 * load older ones again when scrolled up); places opened before that drop
 * their messages and load fresh when opened again. Places never opened
 * through focus (not in `recent`) are left alone. Returns the same record
 * when nothing changed.
 */
export function evicted<M>(
  messages: Record<string, Loaded<M>>,
  recent: readonly string[],
  open: ReadonlySet<string>,
  keepRecent = KEEP_CHANNELS,
  page = KEEP_PAGE,
): Record<string, Loaded<M>> {
  let next = messages;
  let kept = 0;
  for (const at of recent) {
    if (open.has(at)) continue;
    const loaded = next[at];
    if (!loaded || loaded.loading) {
      if (loaded) kept++;
      continue;
    }
    if (kept < keepRecent) {
      kept++;
      const cut = trimmed(loaded, page);
      if (cut !== loaded) next = { ...next, [at]: cut };
    } else {
      const { [at]: _, ...rest } = next;
      next = rest;
    }
  }
  return next;
}

// ───────────────────────── Members ─────────────────────────

/** One collator for every comparison: `localeCompare` makes a new one each call. */
let collator: Intl.Collator | null = null;
const compare = (a: string, b: string) => (collator ??= new Intl.Collator()).compare(a, b);

/** The name a member list sorts by. */
export const memberSortName = (m: Member) => m.nickname || m.user?.displayName || m.user?.username || "";

/** By name; lists that rank people group them by role on top of this. Stable, like the sort it replaces. */
export function sortMembers(members: Member[]): Member[] {
  const keyed = members.map((m, n) => ({ m, n, name: memberSortName(m) }));
  keyed.sort((a, b) => compare(a.name, b.name) || a.n - b.n);
  return keyed.map((k) => k.m);
}

/** Where a member named `name` goes in a sorted list: after everyone named the same, as a sort would put a newcomer. */
function memberIndex(list: readonly Member[], name: string): number {
  let lo = 0;
  let hi = list.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (compare(memberSortName(list[mid]!), name) <= 0) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** A sorted list with one member put in (or moved to) their place, replacing whoever had their user id. */
export function withMember(list: readonly Member[], member: Member): Member[] {
  const id = member.user?.id;
  const at = list.findIndex((m) => m.user?.id === id);
  const rest = list.slice();
  if (at !== -1) rest.splice(at, 1);
  // After everyone with the same name, where re-sorting the list with them last would put them.
  rest.splice(memberIndex(rest, memberSortName(member)), 0, member);
  return rest;
}

// ───────────────────────── Lookups by id ─────────────────────────

/**
 * Members and servers by id, worked out once per list: the store replaces a
 * list whenever it changes, so a list's index stays right for as long as
 * the list lives, and lookups on every event are O(1) instead of a scan.
 */
const memberIndexes = new WeakMap<readonly Member[], Map<string, Member>>();
const serverIndexes = new WeakMap<readonly { id: string }[], Map<string, { id: string }>>();

/** The member with user id `userId` in a server's list. */
export function memberById(list: readonly Member[] | undefined, userId: string | undefined): Member | undefined {
  if (!list || !userId) return undefined;
  let index = memberIndexes.get(list);
  if (!index) {
    index = new Map();
    for (const m of list) if (m.user) index.set(m.user.id, m);
    memberIndexes.set(list, index);
  }
  return index.get(userId);
}

/** The entry with `id` in a list of things with ids (servers). */
export function byId<T extends { id: string }>(list: readonly T[] | undefined, id: string | undefined): T | undefined {
  if (!list || !id) return undefined;
  let index = serverIndexes.get(list);
  if (!index) {
    index = new Map(list.map((x) => [x.id, x]));
    serverIndexes.set(list, index);
  }
  return index.get(id) as T | undefined;
}
