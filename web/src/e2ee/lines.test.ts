import assert from "node:assert/strict";
import { test } from "node:test";
import type { Item } from "./vault";
import { coversUnread, extendWindow, mergeWindow, sameLines, seqIndex, tailOf, touchRecent, upsertLine, windowOf } from "./lines.ts";

const line = (seq: number, kind: Item["kind"] = "text", content = `m${seq}`) =>
  ({ vault: "v", conversation: "c", seq, at: seq, senderId: "u", deviceId: "d", kind, content, replyTo: 0, editedAt: 0, deleted: false, added: [], removed: [] }) as Item;
const seqs = (list: { seq: number }[]) => list.map((i) => i.seq);

test("a tail is the last message and what came after it, without reactions", () => {
  assert.deepEqual(seqs(tailOf([line(1), line(2), line(3, "devices"), line(4, "reaction")])), [2, 3]);
  assert.deepEqual(seqs(tailOf([line(1, "devices")])), [1]);
  assert.deepEqual(tailOf([]), []);
});

test("lines go in their place by seq", () => {
  const list = [line(1), line(3), line(5)];
  assert.equal(seqIndex(list, 6), 3);
  assert.equal(seqIndex(list, 3), 1);
  assert.equal(seqIndex(list, 4), 2);
  assert.deepEqual(seqs(upsertLine(list, line(4))), [1, 3, 4, 5]);
  assert.deepEqual(seqs(upsertLine(list, line(9))), [1, 3, 5, 9]);
  const edited = upsertLine(list, { ...line(3), content: "edited" });
  assert.equal(edited[1]!.content, "edited");
  assert.equal(edited.length, 3);
  assert.equal(upsertLine(list, list[0]!), list);
});

test("new lines land in a window one by one, and older ones wait for it to reach them", () => {
  const w = windowOf([line(10), line(11, "reaction"), line(12)], 10);
  assert.deepEqual(seqs(w.lines), [10, 12]);
  assert.deepEqual(seqs(w.reactions), [11]);
  const next = mergeWindow(w, [line(13), line(14, "reaction"), { ...line(10), content: "edited" }, line(4)]);
  assert.deepEqual(seqs(next.lines), [10, 12, 13]);
  assert.deepEqual(seqs(next.reactions), [11, 14]);
  assert.equal(next.lines[0]!.content, "edited");
  assert.equal(mergeWindow(w, []), w);
  // From the start: everything goes in.
  assert.deepEqual(seqs(mergeWindow(windowOf([line(5)], 0), [line(1)]).lines), [1, 5]);
});

test("a window reaches back, and knows when it holds every unread line", () => {
  const w = windowOf([line(10), line(12)], 10);
  assert.equal(coversUnread(w, 9), true);
  assert.equal(coversUnread(w, 3), false);
  const back = extendWindow(w, [line(7), line(8, "reaction"), line(10)], 7);
  assert.deepEqual(seqs(back.lines), [7, 10, 12]);
  assert.deepEqual(seqs(back.reactions), [8]);
  assert.equal(back.from, 7);
  assert.equal(coversUnread(windowOf([], 0), 0), true);
});

test("conversations opened lately: the latest first, the oldest let go", () => {
  let r = touchRecent([], "a", 2);
  r = touchRecent(r.recent, "b", 2);
  r = touchRecent(r.recent, "c", 2);
  assert.deepEqual(r.recent, ["c", "b", "a"]);
  assert.deepEqual(r.dropped, []);
  r = touchRecent(r.recent, "d", 2);
  assert.deepEqual(r.recent, ["d", "c", "b"]);
  assert.deepEqual(r.dropped, ["a"]);
  r = touchRecent(r.recent, "b", 2);
  assert.deepEqual(r.recent, ["b", "d", "c"]);
  assert.deepEqual(r.dropped, []);
});

test("the same lines read again count as the same", () => {
  assert.equal(sameLines([line(1)], [line(1)]), true);
  assert.equal(sameLines([line(1)], [{ ...line(1), deleted: true }]), false);
  assert.equal(sameLines(undefined, []), false);
});
