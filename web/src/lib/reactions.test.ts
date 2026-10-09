import assert from "node:assert/strict";
import { test } from "node:test";
import {
  isOneEmoji,
  peopleLine,
  reactedWith,
  tallyDm,
  toggled,
  toggledDm,
  withoutReactions,
  withReactionUpdate,
  type ReactionLike,
  type TallyLine,
} from "./reactions.ts";

// Run with `pnpm test` (node's own test runner; no extra dependencies).

const std = (emoji: string, count: number, me = false): ReactionLike => ({ emoji, emojiId: "", emojiName: "", animated: false, count, me });
const custom = (id: string, count: number, me = false): ReactionLike => ({ emoji: "", emojiId: id, emojiName: "party", animated: false, count, me });

test("a ReactionUpdated sets the count it carries, and arriving twice changes nothing", () => {
  let list: ReactionLike[] = [];
  list = withReactionUpdate(list, std("👍", 1), undefined);
  assert.deepEqual(list, [std("👍", 1)]);
  list = withReactionUpdate(list, std("👍", 2), undefined);
  const again = withReactionUpdate(list, std("👍", 2), undefined);
  assert.equal(again, list, "the same event again is the same list");
  assert.equal(list[0]!.count, 2);
  list = withReactionUpdate(list, custom("01EMOJI", 1), undefined);
  assert.deepEqual(
    list.map((r) => r.emoji || r.emojiId),
    ["👍", "01EMOJI"],
    "a new emoji goes last",
  );
});

test("me follows events about you and keeps what it was for others", () => {
  let list: ReactionLike[] = [];
  list = withReactionUpdate(list, std("🎉", 1), true);
  assert.equal(list[0]!.me, true);
  list = withReactionUpdate(list, std("🎉", 2), undefined);
  assert.equal(list[0]!.me, true, "someone else reacting keeps yours");
  list = withReactionUpdate(list, std("🎉", 1), false);
  assert.deepEqual(list, [std("🎉", 1, false)]);
  // Events never carry `me`: it doesn't leak in from one.
  list = withReactionUpdate(list, { ...std("🎉", 2), me: true }, undefined);
  assert.equal(list[0]!.me, false);
});

test("an emoji whose count reaches 0 goes, and only that one", () => {
  const list = [std("👍", 1, true), std("❤️", 3)];
  const next = withReactionUpdate(list, std("👍", 0), false);
  assert.deepEqual(next, [std("❤️", 3)]);
  assert.equal(withReactionUpdate(next, std("👍", 0), false), next, "a removal for an emoji that's gone changes nothing");
});

test("clearing takes one emoji's reactions, or every one", () => {
  const list = [std("👍", 1), custom("01EMOJI", 2), std("❤️", 1)];
  assert.deepEqual(withoutReactions(list, { emoji: "", emojiId: "01EMOJI" }), [std("👍", 1), std("❤️", 1)]);
  assert.deepEqual(withoutReactions(list, { emoji: "👍", emojiId: "" }), [custom("01EMOJI", 2), std("❤️", 1)]);
  assert.deepEqual(withoutReactions(list, { emoji: "", emojiId: "" }), []);
  assert.equal(withoutReactions(list, { emoji: "😮", emojiId: "" }), list);
  const empty: ReactionLike[] = [];
  assert.equal(withoutReactions(empty, { emoji: "", emojiId: "" }), empty, "clearing twice changes nothing");
});

test("toggling your own reaction counts you once, and puts it back exactly", () => {
  const start = [std("👍", 2)];
  const on = toggled(start, std("👍", 1), true);
  assert.deepEqual(on, [std("👍", 3, true)]);
  assert.equal(toggled(on, std("👍", 1), true), on, "already on");
  assert.deepEqual(toggled(on, std("👍", 1), false), [std("👍", 2, false)]);
  assert.deepEqual(toggled([], std("😂", 1), true), [std("😂", 1, true)]);
  assert.deepEqual(toggled([std("😂", 1, true)], std("😂", 1), false), [], "the last one going takes the emoji");
  assert.equal(toggled(start, std("👍", 1), false), start, "taking off one you never put on changes nothing");
  assert.ok(reactedWith(on, { emoji: "👍", emojiId: "" }));
  assert.ok(!reactedWith(start, { emoji: "👍", emojiId: "" }));
});

const line = (seq: number, senderId: string, kind = "text", extra: Partial<TallyLine> = {}): TallyLine => ({ seq, senderId, kind, deleted: false, ...extra });
const reaction = (seq: number, senderId: string, target: number, emoji: string, removed = false): TallyLine =>
  line(seq, senderId, "reaction", { reaction: { target, emoji, removed } });

test("a direct message's tally: the latest reaction from a sender for an emoji wins", () => {
  const lines = [
    line(1, "a"),
    line(2, "b", "voice"),
    reaction(3, "a", 1, "👍"),
    reaction(4, "b", 1, "👍"),
    reaction(5, "b", 1, "❤️"),
    reaction(6, "a", 1, "👍", true),
    reaction(7, "a", 1, "👍"),
    reaction(8, "b", 1, "👍", true),
    reaction(9, "a", 2, "🎉"),
  ];
  const tally = tallyDm(lines, "a");
  assert.deepEqual(
    tally[1]!.map((r) => [r.emoji, r.count, r.me, r.userIds]),
    [
      ["❤️", 1, false, ["b"]],
      ["👍", 1, true, ["a"]],
    ],
  );
  assert.deepEqual(tally[2]!.map((r) => [r.emoji, r.count, r.me]), [["🎉", 1, true]], "voice messages take reactions too");
  // Opened in any order, the same records tally the same; and twice is the same as once.
  assert.deepEqual(tallyDm([...lines].reverse(), "a"), tally);
  assert.deepEqual(tallyDm([...lines, ...lines], "a"), tally);
});

test("an emoji with or without its variation selector is one reaction", () => {
  const tally = tallyDm([line(1, "a"), reaction(2, "a", 1, "👍"), reaction(3, "b", 1, "👍\uFE0F"), reaction(4, "a", 1, "👍\uFE0F", true)], "a");
  assert.deepEqual(tally[1]!.map((r) => [r.emoji, r.count, r.me, r.userIds]), [["👍\uFE0F", 1, false, ["b"]]]);
  const updated = withReactionUpdate([std("👍", 1, true)], std("👍\uFE0F", 2), false);
  assert.deepEqual(updated, [std("👍", 2, false)]);
});

test("reactions to lines that aren't messages, or are gone, or aren't one emoji, are left out", () => {
  const lines = [
    line(1, "a", "devices"),
    line(2, "a", "text", { deleted: true }),
    line(3, "b"),
    reaction(4, "b", 1, "👍"),
    reaction(5, "b", 2, "👍"),
    reaction(6, "b", 99, "👍"),
    reaction(7, "b", 3, "hello"),
    reaction(8, "b", 3, "👍👍"),
    reaction(9, "b", 9, "👍"),
    reaction(10, "a", 3, "🇯🇵"),
  ];
  const tally = tallyDm(lines, "a");
  assert.deepEqual(Object.keys(tally), ["3"]);
  assert.deepEqual(tally[3]!.map((r) => r.emoji), ["🇯🇵"]);
});

test("your reaction in a direct message shows before it's sent, and goes back", () => {
  const before = tallyDm([line(1, "b"), reaction(2, "b", 1, "👍")], "a")[1]!;
  const on = toggledDm(before, "👍", "a", true);
  assert.deepEqual(on.map((r) => [r.count, r.me, r.userIds]), [[2, true, ["b", "a"]]]);
  const off = toggledDm(on, "👍", "a", false);
  assert.deepEqual(off.map((r) => [r.count, r.me, r.userIds]), [[1, false, ["b"]]]);
  assert.deepEqual(toggledDm([], "🔥", "a", true).map((r) => [r.emoji, r.userIds]), [["🔥", ["a"]]]);
});

test("who reacted reads as a short list", () => {
  const words = { separator: ", ", and: (rest: string, last: string) => `${rest} and ${last}`, more: (n: number) => `and ${n} more` };
  assert.equal(peopleLine(["You"], 1, words), "You");
  assert.equal(peopleLine(["You", "Ana"], 2, words), "You and Ana");
  assert.equal(peopleLine(["You", "Ana", "Bo"], 3, words), "You, Ana and Bo");
  assert.equal(peopleLine(["You", "Ana", "Bo", "Cy"], 6, words), "You, Ana, Bo and 3 more");
  assert.equal(peopleLine([], 4, words), "and 4 more");
});

test("only one standard emoji is a reaction", () => {
  for (const ok of ["👍", "❤️", "👍🏽", "🇯🇵", "👩‍👩‍👧", "1️⃣"]) assert.ok(isOneEmoji(ok), ok);
  for (const bad of ["", "a", "👍👍", "hi 👍", "x".repeat(40)]) assert.ok(!isOneEmoji(bad), bad);
});
