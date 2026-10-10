import assert from "node:assert/strict";
import { test } from "node:test";
import type { ChannelHeads } from "@/gen/fuwa/v1/live_pb";
import type { Channel, Message } from "@/gen/fuwa/v1/types_pb";
import type { ChannelMessages, FuwaState, InstanceState } from "./store";
import {
  chooseFocus,
  cursorAfterHeads,
  knownLast,
  MAX_FOCUS_PEOPLE,
  mergeNewest,
  newerHeads,
  NOTIFY_READ_EVERY_MS,
  queueHead,
  readDone,
  readsDue,
  sameFocus,
  touchedBy,
  wantsEveryMessage,
  withHeads,
  type HeadNews,
  type NotifyRead,
} from "./live.ts";

const message = (id: string, extra: Partial<Message> = {}) => ({ id, channelId: "general", content: id, ...extra }) as Message;
const loaded = (ids: string[], hasMore = false): ChannelMessages => ({ items: ids.map((id) => message(id)), hasMore, loading: false });
// Like store.ts's upsertMessage: sorted by id, a newer copy replaces the old one.
const upsert = (items: Message[], m: Message) => [...items.filter((i) => i.id !== m.id), m].sort((a, b) => (a.id < b.id ? -1 : 1));

const instance = (extra: Partial<InstanceState> = {}) =>
  ({
    me: { id: "me" },
    channels: { s1: [{ id: "general" }, { id: "random" }] as Channel[] },
    messages: {},
    unread: {},
    ...extra,
  }) as unknown as InstanceState;

const state = (focus: FuwaState["focus"], inst = instance()): FuwaState => ({ instances: { k: inst }, order: ["k"], focus });

const heads = (servers: { serverId: string; sequence?: bigint; channels: [string, string][] }[]) =>
  ({
    servers: servers.map((s) => ({
      serverId: s.serverId,
      sequence: s.sequence ?? 0n,
      channels: s.channels.map(([channelId, lastMessageId]) => ({ channelId, lastMessageId })),
    })),
  }) as unknown as ChannelHeads;

test("focus is the channel open on this instance and the people shown", () => {
  const focus = chooseFocus(state({ instance: "k", channel: "general", thread: "m1" }), "k", ["b", "a", "b", ""]);
  assert.deepEqual(focus, { channelIds: ["general"], userIds: ["a", "b"] });
});

test("a conversation, another instance or nothing open puts no channel in focus", () => {
  assert.deepEqual(chooseFocus(state({ instance: "k", channel: "a-conversation" }), "k", []).channelIds, []);
  assert.deepEqual(chooseFocus(state({ instance: "other", channel: "general" }), "k", []).channelIds, []);
  assert.deepEqual(chooseFocus(state(null), "k", []).channelIds, []);
});

test("focus names at most 500 people", () => {
  const people = Array.from({ length: 700 }, (_, n) => `u${String(n).padStart(3, "0")}`);
  assert.equal(chooseFocus(state(null), "k", people).userIds.length, MAX_FOCUS_PEOPLE);
});

test("the same focus isn't sent twice", () => {
  const a = { channelIds: ["general"], userIds: ["a", "b"] };
  assert.ok(sameFocus(a, { channelIds: ["general"], userIds: ["a", "b"] }));
  assert.ok(!sameFocus(a, { channelIds: ["general"], userIds: ["a"] }));
  assert.ok(!sameFocus(a, { channelIds: ["random"], userIds: ["a", "b"] }));
  assert.ok(!sameFocus(a, null));
});

test("a cursor moves to the heads' sequence only forward, and only for servers followed", () => {
  assert.equal(cursorAfterHeads(5n, 9n), 9n);
  assert.equal(cursorAfterHeads(12n, 9n), 12n);
  assert.equal(cursorAfterHeads(undefined, 9n), undefined);
});

test("heads newer than what's known light unread marks out of focus", () => {
  const inst = instance({ messages: { general: loaded(["m1", "m3"]) }, unread: { random: 2 } });
  const known = new Map([["random", "m5"]]);
  const news = newerHeads(
    inst,
    heads([{ serverId: "s1", channels: [["general", "m4"], ["random", "m5"], ["lounge", "m2"]] }]),
    known,
  );
  assert.deepEqual(news, [
    { serverId: "s1", channelId: "general", lastMessageId: "m4", after: "m3" },
    { serverId: "s1", channelId: "lounge", lastMessageId: "m2", after: undefined },
  ]);
  const next = withHeads(inst, news, "general");
  assert.deepEqual(next.unread, { random: 2, lounge: 1 });
  assert.equal(withHeads(inst, [], null), inst);
});

test("what's known of a channel is the newer of what's loaded and its last head", () => {
  const inst = instance({ messages: { general: loaded(["m1", "m3"]) } });
  assert.equal(knownLast(inst, new Map(), "general"), "m3");
  assert.equal(knownLast(inst, new Map([["general", "m7"]]), "general"), "m7");
  assert.equal(knownLast(inst, new Map([["general", "m2"]]), "general"), "m3");
  assert.equal(knownLast(inst, new Map(), "random"), undefined);
});

test("a re-read page that overlaps the cache is what's true from its oldest message on", () => {
  const cached = { ...loaded(["m1", "m2", "m3", "m4"], true) };
  cached.items[3] = message("m4", { content: "before the edit" });
  const page = { messages: [message("m3"), message("m4", { content: "edited" }), message("m6")], hasMore: true };
  const merged = mergeNewest(cached, page, upsert);
  // m6 came out of focus and m4 was edited; m1 and m2 are older than the page and stay.
  assert.deepEqual(
    merged.items.map((m) => m.id),
    ["m1", "m2", "m3", "m4", "m6"],
  );
  assert.equal(merged.items[3]!.content, "edited");
  assert.equal(merged.hasMore, true);
});

test("messages deleted out of focus go when the page no longer has them", () => {
  const merged = mergeNewest(loaded(["m1", "m2", "m3"]), { messages: [message("m1"), message("m3")], hasMore: false }, upsert);
  assert.deepEqual(
    merged.items.map((m) => m.id),
    ["m1", "m3"],
  );
  assert.equal(merged.hasMore, false);
});

test("a page that doesn't reach back to the cache replaces it", () => {
  const merged = mergeNewest(loaded(["m1", "m2"]), { messages: [message("m7"), message("m8")], hasMore: true }, upsert);
  assert.deepEqual(
    merged.items.map((m) => m.id),
    ["m7", "m8"],
  );
  assert.equal(merged.hasMore, true);
});

test("an emptied channel, and a first load still on its way", () => {
  assert.deepEqual(mergeNewest(loaded(["m1"]), { messages: [], hasMore: false }, upsert).items, []);
  const loading = mergeNewest({ items: [], hasMore: false, loading: true }, { messages: [message("m1")], hasMore: false }, upsert);
  assert.deepEqual(
    loading.items.map((m) => m.id),
    ["m1"],
  );
  assert.equal(loading.loading, true);
});

test("only channels where every message notifies are read for notifications", () => {
  assert.ok(wantsEveryMessage({ level: 1, muted: false }, "mentions"));
  assert.ok(wantsEveryMessage({ level: 0, muted: false }, "all"));
  assert.ok(!wantsEveryMessage({ level: 0, muted: false }, "mentions"));
  assert.ok(!wantsEveryMessage({ level: 2, muted: false }, "all"));
  assert.ok(!wantsEveryMessage({ level: 3, muted: false }, "all"));
  assert.ok(!wantsEveryMessage({ level: 1, muted: true }, "all"));
});

const during = (changed: string[] = [], deleted: string[] = []) => ({ changed: new Set(changed), deleted: new Set(deleted) });
const ids = (c: ChannelMessages) => c.items.map((m) => m.id);

test("messages that came live while the page was read stay, newer than it or changed meanwhile", () => {
  const cached = loaded(["m1", "m2", "m3", "m9"]);
  cached.items[2] = message("m3", { content: "edited live" });
  const page = { messages: [message("m2"), message("m3", { content: "before the edit" }), message("m4")], hasMore: true };
  const merged = mergeNewest(cached, page, upsert, during(["m3", "m9"]));
  assert.deepEqual(ids(merged), ["m1", "m2", "m3", "m4", "m9"]);
  assert.equal(merged.items[2]!.content, "edited live");
});

test("a message deleted while the page was read stays gone", () => {
  const merged = mergeNewest(loaded(["m1", "m3"]), { messages: [message("m1"), message("m2"), message("m3")], hasMore: false }, upsert, during([], ["m2"]));
  assert.deepEqual(ids(merged), ["m1", "m3"]);
});

test("an empty page keeps what came live meanwhile", () => {
  const merged = mergeNewest(loaded(["m1", "m5"]), { messages: [], hasMore: false }, upsert, during(["m5"]));
  assert.deepEqual(ids(merged), ["m5"]);
});

test("a gap keeps what came live meanwhile, past the page", () => {
  const cached = loaded(["m1", "m2", "m9"]);
  // m9 is newer than the cache's other messages: it came live after the read went out.
  const merged = mergeNewest(cached, { messages: [message("m7"), message("m8")], hasMore: true }, upsert, during(["m9"]));
  assert.deepEqual(ids(merged), ["m7", "m8", "m9"]);
  assert.equal(merged.hasMore, true);
});

test("what live events a re-read could undo", () => {
  const ev = (c: string, value: object) => ({ payload: { case: c, value } }) as never;
  assert.deepEqual(touchedBy(ev("messageCreated", { message: { id: "m1", channelId: "c" } })), { channelId: "c", messageId: "m1", deleted: false });
  assert.deepEqual(touchedBy(ev("messageDeleted", { channelId: "c", messageId: "m1" })), { channelId: "c", messageId: "m1", deleted: true });
  assert.deepEqual(touchedBy(ev("reactionUpdated", { channelId: "c", messageId: "m1" })), { channelId: "c", messageId: "m1", deleted: false });
  assert.deepEqual(touchedBy(ev("threadUpdated", { channelId: "c", threadId: "m1" })), { channelId: "c", messageId: "m1", deleted: false });
  assert.equal(touchedBy(ev("memberJoined", {})), null);
});

const news = (channelId: string, lastMessageId: string, after?: string): HeadNews => ({ serverId: "s1", channelId, lastMessageId, after });

test("heads for notifications fold into one read from where the last got to", () => {
  const reads = new Map<string, NotifyRead>();
  queueHead(reads, news("a", "m5", "m3"));
  assert.deepEqual(readsDue(reads, 100_000).start, ["a"]);
  const r = reads.get("a")!;
  r.busy = true;
  r.lastRead = 100_000;
  // Two more heads while the read is out: neither is dropped, both wait for one read.
  queueHead(reads, news("a", "m7", "m5"));
  queueHead(reads, news("a", "m8", "m7"));
  assert.deepEqual(readsDue(reads, 100_500), { start: [], wake: null });
  readDone(reads, "a", "m5", "m5");
  assert.equal(r.from, "m5");
  assert.equal(r.head, "m8");
  // Not again for ten seconds, then once, from m5.
  assert.deepEqual(readsDue(reads, 101_000), { start: [], wake: 100_000 + NOTIFY_READ_EVERY_MS });
  assert.deepEqual(readsDue(reads, 100_000 + NOTIFY_READ_EVERY_MS).start, ["a"]);
  readDone(reads, "a", "m8", "m8");
  assert.deepEqual(readsDue(reads, 200_000), { start: [], wake: null });
});

test("at most two reads for notifications at once", () => {
  const reads = new Map<string, NotifyRead>();
  for (const c of ["a", "b", "c"]) queueHead(reads, news(c, "m2", "m1"));
  const { start } = readsDue(reads, 50_000);
  assert.equal(start.length, 2);
  for (const c of start) reads.get(c)!.busy = true;
  assert.deepEqual(readsDue(reads, 50_000).start, []);
  readDone(reads, start[0]!, "m2", "m2");
  assert.equal(readsDue(reads, 50_000).start.length, 1);
});

test("a read that found nothing still counts up to its head", () => {
  const reads = new Map<string, NotifyRead>();
  queueHead(reads, news("a", "m4"));
  readDone(reads, "a", undefined, "m4");
  assert.equal(reads.get("a")!.from, "m4");
});
