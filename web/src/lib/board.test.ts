import assert from "node:assert/strict";
import { test } from "node:test";
import type { BoardColumn, Card, Event, Message } from "@/gen/fuwa/v1/types_pb";
import { applyBoard, between, boardOf, cardsIn, dueState, moved, rankFor, shows, step, type BoardState } from "./board.ts";

// Run with `pnpm test` (node's own test runner; no extra dependencies).

const column = (id: string, done = false) => ({ id, name: id, color: 0, done, cardLimit: 0 }) as BoardColumn;
const card = (id: string, columnId: string, rank: string, extra: Partial<Card> = {}) =>
  ({ id, channelId: "b", columnId, rank, title: id, labelIds: [], assigneeIds: [], authorId: "u", ...extra }) as Card;
const body = (id: string, content = "") => ({ id, channelId: "b", content, threadId: "", reactions: [] }) as unknown as Message;
const titles = (board: BoardState, columnId: string) => cardsIn(board, columnId).map((c) => c.id);
const payload = (p: unknown) => p as Event["payload"];

const start = boardOf([column("todo"), column("doing"), column("done", true)], [], [card("b1", "todo", "V"), card("a1", "todo", "k"), card("c1", "doing", "V")], [body("a1")]);

test("ranks fall between their neighbours, and never end in 0", () => {
  const ranks = [between("", "")];
  for (let i = 0; i < 100; i++) ranks.push(between(ranks.at(-1)!, ""));
  for (let i = 0; i < 100; i++) ranks.unshift(between("", ranks[0]!));
  for (let i = 0; i < 100; i++) ranks.splice(150, 0, between(ranks[149]!, ranks[150]!));
  for (let i = 1; i < ranks.length; i++) {
    assert.ok(ranks[i - 1]! < ranks[i]!, `${ranks[i - 1]} < ${ranks[i]}`);
    assert.ok(!ranks[i]!.endsWith("0"));
  }
  // The same as the server's (server/src/api/boards.rs).
  assert.equal(between("", ""), "V");
  assert.equal(between("V", "V1"), "V0V");
  assert.equal(between("z", ""), "zV");
});

test("a drop lands above the card named, or at the bottom", () => {
  assert.deepEqual(titles(moved(start, "c1", "todo", "a1"), "todo"), ["b1", "c1", "a1"]);
  assert.deepEqual(titles(moved(start, "b1", "todo", ""), "todo"), ["a1", "b1"]);
  assert.deepEqual(titles(moved(start, "a1", "done", ""), "done"), ["a1"]);
  assert.equal(rankFor(start, "doing", "", "x") > "V", true);
});

test("the keyboard moves a card a place, or to the next column's top", () => {
  assert.deepEqual(step(start, "b1", "down"), { columnId: "todo", beforeId: "" });
  assert.deepEqual(step(start, "a1", "up"), { columnId: "todo", beforeId: "b1" });
  assert.equal(step(start, "b1", "up"), null);
  assert.deepEqual(step(start, "b1", "right"), { columnId: "doing", beforeId: "c1" });
  assert.equal(step(start, "b1", "left"), null);
});

test("events keep a board in step, and coming twice changes nothing", () => {
  const updated = payload({ case: "cardUpdated", value: { card: card("c1", "todo", "1") } });
  const once = applyBoard(start, updated);
  assert.deepEqual(titles(once, "todo"), ["c1", "b1", "a1"]);
  assert.deepEqual(applyBoard(once, updated), once);
  // Archived cards leave the board.
  const archived = applyBoard(once, payload({ case: "cardUpdated", value: { card: card("c1", "todo", "1", { archivedAt: { seconds: 5n, nanos: 0 } as never }) } }));
  assert.deepEqual(titles(archived, "todo"), ["b1", "a1"]);
  // A body comes as a message; comments don't touch it.
  const withBody = applyBoard(start, payload({ case: "messageCreated", value: { message: body("b1", "hello") } }));
  assert.equal(withBody.bodies.b1?.content, "hello");
  const comment = applyBoard(withBody, payload({ case: "messageCreated", value: { message: { ...body("x", "hi"), threadId: "b1" } } }));
  assert.equal(comment, withBody);
  const thread = applyBoard(withBody, payload({ case: "threadUpdated", value: { channelId: "b", threadId: "b1", thread: { replyCount: 3 } } }));
  assert.equal(thread.bodies.b1?.thread?.replyCount, 3);
  // Deleting the body takes the card.
  const gone = applyBoard(withBody, payload({ case: "messageDeleted", value: { channelId: "b", messageId: "b1" } }));
  assert.deepEqual(titles(gone, "todo"), ["a1"]);
  assert.equal(gone.bodies.b1, undefined);
  // Labels removed come off cards.
  const labelled = applyBoard(start, payload({ case: "cardUpdated", value: { card: card("a1", "todo", "k", { labelIds: ["l1", "l2"] }) } }));
  const relabelled = applyBoard(labelled, payload({ case: "boardUpdated", value: { channelId: "b", columns: start.columns, labels: [{ id: "l2", name: "x", color: 1 }] } }));
  assert.deepEqual(relabelled.cards.find((c) => c.id === "a1")?.labelIds, ["l2"]);
});

test("filters and due dates", () => {
  const c = card("a", "todo", "V", { title: "Fix the overlay", assigneeIds: ["rin"], labelIds: ["bug"], dueAt: { seconds: 100n, nanos: 0 } as never });
  assert.ok(shows(c, body("a", "flickers on Wayland"), { query: "overlay wayland" }));
  assert.ok(!shows(c, undefined, { query: "wayland" }));
  assert.ok(shows(c, undefined, { assignee: "rin", labelId: "bug" }));
  assert.ok(!shows(c, undefined, { assignee: "mika" }));
  assert.equal(dueState(c, column("todo"), 200_000), "late");
  assert.equal(dueState(c, column("done", true), 200_000), "later");
  assert.equal(dueState(c, column("todo"), 50_000), "soon");
  assert.equal(dueState(card("b", "todo", "V"), column("todo"), 0), null);
});
