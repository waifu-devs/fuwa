import assert from "node:assert/strict";
import { test } from "node:test";
import { archived, deleteLine, following, forgetLine, organize, orphaned, search, threadOf, unreadIn } from "./threads.ts";
import type { Item } from "./vault.ts";

function line(seq: number, senderId: string, content: string, extra: Partial<Item> = {}): Item {
  return {
    vault: "v",
    conversation: "c",
    seq,
    at: seq * 1000,
    senderId,
    deviceId: `${senderId}-d`,
    kind: "text",
    content,
    replyTo: 0,
    editedAt: 0,
    deleted: false,
    added: [],
    removed: [],
    ...extra,
  };
}

const lock = (seq: number, senderId: string, parent: number, locked = true) =>
  line(seq, senderId, locked ? "locked" : "unlocked", {
    kind: "thread",
    thread: parent,
  });

const mods = (userId: string) => userId === "mod";

test("replies leave the channel unless also sent there, and threads sum up their replies", () => {
  const items = [
    line(1, "aoi", "anyone up for games?"),
    line(2, "mika", "me!", { thread: 1 }),
    line(3, "juan", "count me in", { thread: 1, inChannel: true }),
    line(4, "aoi", "unrelated"),
    line(5, "mika", "8pm?", { thread: 1 }),
  ];
  const org = organize(items, mods);
  assert.deepEqual(
    org.channel.map((i) => i.seq),
    [1, 3, 4],
  );
  const t = org.threads.get(1)!;
  assert.equal(t.replies, 3);
  assert.equal(t.lastSeq, 5);
  assert.deepEqual(t.participants, ["mika", "juan"]);
  assert.deepEqual(
    org.inThread.get(1)!.map((i) => i.seq),
    [2, 3, 5],
  );
});

test("a reply under something that can't have a thread shows as an ordinary line", () => {
  const items = [
    line(1, "aoi", "", { kind: "devices" }),
    line(2, "mika", "under a device line", { thread: 1 }),
    line(3, "aoi", "parent"),
    line(4, "mika", "reply", { thread: 3 }),
    line(5, "juan", "nested", { thread: 4 }),
    line(6, "juan", "from the future", { thread: 9 }),
  ];
  const bySeq = new Map(items.map((i) => [i.seq, i]));
  assert.equal(threadOf(items[1], bySeq), 0);
  assert.equal(threadOf(items[4], bySeq), 0);
  assert.equal(threadOf(items[5], bySeq), 0);
  assert.deepEqual(
    organize(items, mods).channel.map((i) => i.seq),
    [1, 2, 3, 5, 6],
  );
});

test("a reply whose message isn't on this device stays in its thread", () => {
  const items = [line(7, "mika", "reply to something older", { thread: 3 })];
  const org = organize(items, mods);
  assert.equal(org.channel.length, 0);
  assert.equal(org.threads.get(3)?.replies, 1);
});

test("locks count only from moderators, and the latest wins", () => {
  const items = [
    line(1, "aoi", "parent"),
    line(2, "mika", "r", { thread: 1 }),
    lock(3, "aoi", 1),
    lock(4, "mod", 1),
    lock(5, "mod", 1, false),
    lock(6, "mod", 1),
  ];
  const org = organize(items, mods);
  assert.equal(org.threads.get(1)!.locked, true);
  assert.equal(organize(items.slice(0, 5), mods).threads.get(1)!.locked, false);
  assert.equal(organize(items.slice(0, 3), mods).threads.get(1)!.locked, false);
  assert.ok(!org.channel.some((i) => i.kind === "thread"));
});

test("deleting a thread's message drops its replies on this device only", () => {
  const items = [line(1, "aoi", "", { deleted: true }), line(2, "mika", "r", { thread: 1 }), line(3, "juan", "other", { thread: 9 })];
  const dropped = orphaned(items, new Map(items.map((i) => [i.seq, i])));
  assert.deepEqual(
    dropped.map((i) => [i.seq, i.deleted, i.content]),
    [[2, true, ""]],
  );
});

test("following: by hand, or by writing the message or a reply", () => {
  const items = [line(1, "aoi", "parent"), line(2, "mika", "r", { thread: 1 })];
  assert.equal(following(undefined, 1, items, "aoi"), true);
  assert.equal(following(undefined, 1, items, "mika"), true);
  assert.equal(following(undefined, 1, items, "juan"), false);
  assert.equal(following({ follows: { 1: true } }, 1, items, "juan"), true);
  assert.equal(following({ follows: { 1: false } }, 1, items, "aoi"), false);
});

test("unread counts others' replies after the last one seen", () => {
  const replies = [line(2, "mika", "a", { thread: 1 }), line(3, "aoi", "b", { thread: 1 }), line(4, "mika", "c", { thread: 1 })];
  assert.equal(unreadIn(undefined, 1, replies, "aoi"), 2);
  assert.equal(unreadIn({ threadRead: { 1: 2 } }, 1, replies, "aoi"), 1);
});

test("archiving and search work from what's on the device", () => {
  const items = [
    line(1, "aoi", "games tonight"),
    line(2, "mika", "pizza after?", { thread: 1 }),
    line(3, "juan", "movies"),
    line(4, "aoi", "ok", { thread: 3 }),
  ];
  const org = organize(items, mods);
  const bySeq = new Map(items.map((i) => [i.seq, i]));
  assert.deepEqual(
    search(org, bySeq, "").map((t) => t.parent),
    [3, 1],
  );
  assert.deepEqual(
    search(org, bySeq, "PIZZA").map((t) => t.parent),
    [1],
  );
  const t = org.threads.get(1)!;
  assert.equal(archived(t, 1, t.lastAt + 3_600_001), true);
  assert.equal(archived(t, 0, t.lastAt + 1e12), false);
});

test("a deleted message's thread doesn't show, even before its replies are dropped", () => {
  const items = [line(1, "aoi", "", { deleted: true }), line(2, "mika", "r", { thread: 1, inChannel: true })];
  const org = organize(items, mods);
  assert.deepEqual(
    org.channel.map((i) => i.seq),
    [1],
  );
  assert.equal(org.threads.size, 0);
});

test("threads never ask the server for anything: no record is fetched to fill a thread in", async () => {
  const { readFile } = await import("node:fs/promises");
  for (const file of ["./threads.ts", "../components/chat/SecureThreads.tsx"]) {
    const source = await readFile(new URL(file, import.meta.url), "utf8");
    for (const call of ["listSecureRecords", "records(", "api.", "fetch(", "@/fuwa/client", "@/fuwa/actions"]) {
      assert.ok(!source.includes(call), `${file} mentions ${call}`);
    }
  }
});

/** A stand-in for the server and the vault that counts what's asked of the server. */
function mockChannel(items: Item[]) {
  const asked = { removed: [] as number[], listed: 0 };
  let stored = items;
  const io = {
    remove: async (seq: number) => {
      asked.removed.push(seq);
    },
    // Anything that lists records would come through here; nothing should.
    records: async () => {
      asked.listed++;
      return [];
    },
    locked: async (fn: () => Promise<void>) => fn(),
    load: async () => stored,
    write: async (changed: Item[]) => {
      const bySeq = new Map(stored.map((i) => [i.seq, i]));
      for (const i of changed) bySeq.set(i.seq, i);
      stored = [...bySeq.values()].sort((a, b) => a.seq - b.seq);
    },
  };
  return { io, asked, lines: () => stored };
}

test("deleting a thread's message makes one delete on the server and lists nothing", async () => {
  const items = [
    line(1, "aoi", "parent"),
    line(2, "mika", "a", { thread: 1 }),
    line(3, "juan", "b", { thread: 1, inChannel: true }),
    line(4, "aoi", "other"),
  ];
  const { io, asked, lines } = mockChannel(items);
  await deleteLine(io, 1, true);
  assert.deepEqual(asked.removed, [1]);
  assert.equal(asked.listed, 0);
  assert.deepEqual(
    lines().map((i) => [i.seq, i.deleted]),
    [
      [1, true],
      [2, true],
      [3, true],
      [4, false],
    ],
  );
});

test("someone else deleting a thread's message asks the server nothing", async () => {
  const items = [line(1, "aoi", "parent"), line(2, "mika", "a", { thread: 1 })];
  const { io, asked, lines } = mockChannel(items);
  await forgetLine(io, 1, true);
  assert.deepEqual(asked.removed, []);
  assert.equal(asked.listed, 0);
  assert.ok(lines().every((i) => i.deleted));
});

test("opening a thread whose message isn't on this device works from what's here", () => {
  const { asked } = mockChannel([]);
  const items = [line(7, "mika", "reply to something older", { thread: 3 })];
  const org = organize(items, mods);
  assert.equal(org.inThread.get(3)?.length, 1);
  assert.equal(search(org, new Map(items.map((i) => [i.seq, i])), "older").length, 1);
  assert.equal(asked.listed, 0);
});

test("a lock on a deleted or missing message makes no thread", () => {
  assert.equal(organize([line(1, "aoi", "", { deleted: true }), lock(2, "mod", 1)], mods).threads.size, 0);
  assert.equal(organize([lock(5, "mod", 3)], mods).threads.size, 0);
  assert.equal(organize([line(1, "aoi", "parent"), lock(2, "mod", 1)], mods).threads.get(1)?.locked, true);
});
