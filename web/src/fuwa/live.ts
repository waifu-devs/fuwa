import type { ChannelHeads } from "@/gen/fuwa/v1/live_pb";
import type { Event, Message } from "@/gen/fuwa/v1/types_pb";
import type { ChannelMessages, FuwaState, InstanceState } from "./store";

/**
 * The pure half of a live connection (docs/live.md): what to put in focus,
 * what channel heads change, how a re-read page meets what's cached, and
 * when a head is worth reading for a notification. `sync.ts` runs the
 * connection itself. Only types come from elsewhere, so node's test runner
 * reads this file as is.
 */

/** The most channels and people a Focus may name. */
export const MAX_FOCUS_CHANNELS = 8;
export const MAX_FOCUS_PEOPLE = 500;

export type LiveFocus = { channelIds: string[]; userIds: string[] };

/**
 * What an instance's connection should have in focus: the channel open there
 * (a thread open beside it comes with it, since replies come with their
 * channel) and the people on screen, which are the ones whose presence
 * something shows. A conversation on screen isn't a channel and isn't named.
 */
export function chooseFocus(s: FuwaState, key: string, people: Iterable<string>): LiveFocus {
  const inst = s.instances[key];
  const open = s.focus?.instance === key ? s.focus.channel : null;
  const channelIds =
    inst && open && Object.values(inst.channels).some((list) => list.some((c) => c.id === open)) ? [open] : [];
  const userIds = [...new Set(people)].filter(Boolean).sort().slice(0, MAX_FOCUS_PEOPLE);
  return { channelIds: channelIds.slice(0, MAX_FOCUS_CHANNELS), userIds };
}

export const sameFocus = (a: LiveFocus | null, b: LiveFocus | null) =>
  !!a &&
  !!b &&
  a.channelIds.length === b.channelIds.length &&
  a.userIds.length === b.userIds.length &&
  a.channelIds.every((id, n) => id === b.channelIds[n]) &&
  a.userIds.every((id, n) => id === b.userIds[n]);

/** A server's cursor after its heads: the higher of the two. Unknown servers stay unknown. */
export const cursorAfterHeads = (cursor: bigint | undefined, sequence: bigint) =>
  cursor === undefined || sequence <= cursor ? cursor : sequence;

/** One channel that moved past what this app knew of it, and where it knew it up to. */
export type HeadNews = { serverId: string; channelId: string; lastMessageId: string; after: string | undefined };

/** The newest message this app knows in a channel: the newest loaded, or the last head or read for it. */
export function knownLast(i: InstanceState, known: ReadonlyMap<string, string>, channelId: string): string | undefined {
  const loaded = i.messages[channelId]?.items.at(-1)?.id;
  const seen = known.get(channelId);
  if (loaded === undefined) return seen;
  return seen === undefined || loaded > seen ? loaded : seen;
}

/** The channel heads newer than what's known (ids sort by time). */
export function newerHeads(i: InstanceState, heads: ChannelHeads, known: ReadonlyMap<string, string>): HeadNews[] {
  const news: HeadNews[] = [];
  for (const server of heads.servers) {
    for (const head of server.channels) {
      const after = knownLast(i, known, head.channelId);
      if (head.lastMessageId && (after === undefined || head.lastMessageId > after)) {
        news.push({ serverId: server.serverId, channelId: head.channelId, lastMessageId: head.lastMessageId, after });
      }
    }
  }
  return news;
}

/**
 * Lights the unread marks of channels that moved out of focus. A head says
 * only where a channel stands, not how many came, so each counts one more.
 */
export function withHeads(i: InstanceState, news: HeadNews[], focusChannel: string | null): InstanceState {
  let unread = i.unread;
  for (const n of news) {
    if (n.channelId === focusChannel) continue;
    unread = { ...unread, [n.channelId]: (unread[n.channelId] ?? 0) + 1 };
  }
  return unread === i.unread ? i : { ...i, unread };
}

/** What happened live to a channel's messages while its page was being read again. */
export type DuringRead = { changed: ReadonlySet<string>; deleted: ReadonlySet<string> };
const NOTHING_DURING: DuringRead = { changed: new Set(), deleted: new Set() };

/**
 * The message a live event changes, if it's one a re-read could undo: made,
 * edited, reacted to, voted on, pinned or given a thread (`deleted` when it
 * went). A thread's replies count under their channel.
 */
export function touchedBy(event: Pick<Event, "payload">): { channelId: string; messageId: string; deleted: boolean } | null {
  const p = event.payload;
  switch (p.case) {
    case "messageCreated":
    case "messageUpdated":
      return p.value.message ? { channelId: p.value.message.channelId, messageId: p.value.message.id, deleted: false } : null;
    case "messageDeleted":
      return { channelId: p.value.channelId, messageId: p.value.messageId, deleted: true };
    case "reactionUpdated":
    case "reactionsCleared":
    case "pollUpdated":
    case "messagePinned":
      return { channelId: p.value.channelId, messageId: p.value.messageId, deleted: false };
    case "threadUpdated":
      return { channelId: p.value.channelId, messageId: p.value.threadId, deleted: false };
    default:
      return null;
  }
}

/**
 * A channel's newest page, read again after it came into focus, against what
 * was cached. Within the page's span the page is what's true (edits,
 * deletions, reactions and polls out of focus never came), and older cached
 * messages stay. When the page doesn't reach back to anything cached there's
 * a gap, so the page takes the cache's place. Either way what came live
 * while the page was on its way wins over it (`during`): messages newer than
 * the page, or made or changed meanwhile, stay as they are, and ones deleted
 * meanwhile stay gone.
 */
export function mergeNewest(
  cached: ChannelMessages | undefined,
  page: { messages: Message[]; hasMore: boolean },
  upsert: (items: Message[], message: Message) => Message[],
  during: DuringRead = NOTHING_DURING,
): ChannelMessages {
  let oldest: string | undefined;
  let newest: string | undefined;
  for (const m of page.messages) {
    if (oldest === undefined || m.id < oldest) oldest = m.id;
    if (newest === undefined || m.id > newest) newest = m.id;
  }
  const items = cached?.items ?? [];
  const live = (m: Message) => during.changed.has(m.id) || (newest !== undefined && m.id > newest);
  // Whether the page reaches back to the cache goes by what was cached before it went out.
  const newestCached = items.findLast((m) => !live(m))?.id;
  const gap = !items.length || (page.hasMore && oldest !== undefined && (newestCached === undefined || newestCached < oldest));
  // A first load still on its way keeps its flag, and lands on top of this.
  const loading = cached?.loading ?? false;
  const ids = new Set(page.messages.map((m) => m.id));
  const older = (m: Message) => !gap && oldest !== undefined && m.id < oldest;
  const kept = items.filter((m) => !during.deleted.has(m.id) && (older(m) || live(m) || (!gap && ids.has(m.id))));
  const ours = new Set(kept.filter(live).map((m) => m.id));
  const fresh = page.messages.filter((m) => !during.deleted.has(m.id) && !ours.has(m.id));
  return { items: fresh.reduce(upsert, kept), hasMore: kept.some(older) ? (cached?.hasMore ?? false) : page.hasMore, loading };
}

// NotificationLevel's values (types_pb), so node's test runner reads this file as is.
const ALL = 1;
const UNSPECIFIED = 0;

/**
 * Whether a channel's notification settings ask for every message, not just
 * mentions: those are the channels whose heads are read for notifications.
 */
export const wantsEveryMessage = (e: { level: number; muted: boolean }, notifyFor: "all" | "mentions") =>
  !e.muted && (e.level === ALL || (e.level === UNSPECIFIED && notifyFor === "all"));

/** At most this many reads for notifications at once per instance, and one per channel this often. */
export const NOTIFY_READS_AT_ONCE = 2;
export const NOTIFY_READ_EVERY_MS = 10_000;

/**
 * Where reading a channel for notifications stands: messages after `from`
 * (none known yet: the newest page) up to its latest head are still to read.
 */
export type NotifyRead = { serverId: string; from: string | undefined; head: string; lastRead: number; busy: boolean };

/**
 * A head for a channel out of focus where every message notifies. Heads that
 * come while a read is going or too soon after one fold into the next read,
 * which starts from where the last one got to, so none are skipped.
 */
export function queueHead(reads: Map<string, NotifyRead>, news: HeadNews) {
  const r = reads.get(news.channelId);
  if (!r) reads.set(news.channelId, { serverId: news.serverId, from: news.after, head: news.lastMessageId, lastRead: 0, busy: false });
  else if (news.lastMessageId > r.head) r.head = news.lastMessageId;
}

/**
 * The reads to start now (no more than NOTIFY_READS_AT_ONCE going, each
 * channel at most every NOTIFY_READ_EVERY_MS), and when to look again for
 * ones held back by that wait (null: nothing waits on time).
 */
export function readsDue(reads: ReadonlyMap<string, NotifyRead>, now: number): { start: string[]; wake: number | null } {
  let going = [...reads.values()].filter((r) => r.busy).length;
  const start: string[] = [];
  let wake: number | null = null;
  const waiting = [...reads.entries()].filter(([, r]) => !r.busy && r.head > (r.from ?? "")).sort(([, a], [, b]) => a.lastRead - b.lastRead);
  for (const [channelId, r] of waiting) {
    const at = r.lastRead + NOTIFY_READ_EVERY_MS;
    if (at > now) wake = wake === null ? at : Math.min(wake, at);
    else if (going < NOTIFY_READS_AT_ONCE) {
      start.push(channelId);
      going++;
    }
  }
  return { start, wake };
}

/**
 * A read for notifications finished: it got up to `newest` (the newest
 * message it read), or, having read nothing, to the head it set out for.
 */
export function readDone(reads: Map<string, NotifyRead>, channelId: string, newest: string | undefined, headAtStart: string) {
  const r = reads.get(channelId);
  if (!r) return;
  r.busy = false;
  const upTo = newest ?? headAtStart;
  if (r.from === undefined || upTo > r.from) r.from = upTo;
}
