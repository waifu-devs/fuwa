import assert from "node:assert/strict";
import { test } from "node:test";
import { added, dismissed, type Ephemerals, expire, type Kept, merged, minutesLeft, nextExpiry, nextTick } from "./ephemeral.ts";

type M = { id: string; at: number };

const NOW = 1_800_000_000_000;
const MIN = 60_000;
const kept = (id: string, at: number, until: number): Kept<M> => ({ message: { id, at }, at, until });
const ids = (state: Ephemerals<M>, place: string) => (state[place] ?? []).map((k) => k.message.id);

test("added keeps them in time order, replaces the same id and skips one already out of time", () => {
  let s: Ephemerals<M> = {};
  s = added(s, "c1", kept("b", NOW, NOW + 15 * MIN), NOW);
  s = added(s, "c1", kept("a", NOW - 1000, NOW + 15 * MIN), NOW);
  s = added(s, "c1", kept("c", NOW + 1000, NOW + 15 * MIN), NOW);
  assert.deepEqual(ids(s, "c1"), ["a", "b", "c"]);
  s = added(s, "c1", kept("b", NOW, NOW + 20 * MIN), NOW);
  assert.deepEqual(ids(s, "c1"), ["a", "b", "c"]);
  assert.equal(s.c1[1].until, NOW + 20 * MIN);
  const before = s;
  assert.equal(added(s, "c1", kept("d", NOW, NOW), NOW), before);
  assert.equal(added(s, "c2", kept("d", NOW, NOW - 1), NOW), before);
});

test("dismissed takes one off, and the place when it was the last", () => {
  let s: Ephemerals<M> = {};
  s = added(s, "c1", kept("a", NOW, NOW + MIN), NOW);
  s = added(s, "c1", kept("b", NOW + 1, NOW + MIN), NOW);
  assert.equal(dismissed(s, "c1", "x"), s);
  assert.equal(dismissed(s, "c9", "a"), s);
  s = dismissed(s, "c1", "a");
  assert.deepEqual(ids(s, "c1"), ["b"]);
  s = dismissed(s, "c1", "b");
  assert.deepEqual(Object.keys(s), []);
});

test("expire drops what ran out and keeps unchanged places as they were", () => {
  let s: Ephemerals<M> = {};
  s = added(s, "c1", kept("a", NOW, NOW + MIN), NOW);
  s = added(s, "c1", kept("b", NOW + 1, NOW + 5 * MIN), NOW);
  s = added(s, "c2", kept("c", NOW, NOW + 10 * MIN), NOW);
  assert.equal(nextExpiry(s), NOW + MIN);
  assert.equal(expire(s, NOW), s);
  const later = expire(s, NOW + MIN);
  assert.deepEqual(ids(later, "c1"), ["b"]);
  assert.equal(later.c2, s.c2);
  assert.equal(nextExpiry(later), NOW + 5 * MIN);
  const gone = expire(later, NOW + 10 * MIN);
  assert.deepEqual(gone, {});
  assert.equal(nextExpiry(gone), null);
});

test("minutes left round up, and under a minute says so", () => {
  assert.equal(minutesLeft(NOW + 15 * MIN, NOW), 15);
  assert.equal(minutesLeft(NOW + 14 * MIN + 1, NOW), 15);
  assert.equal(minutesLeft(NOW + 2 * MIN, NOW), 2);
  assert.equal(minutesLeft(NOW + MIN + 1, NOW), 2);
  assert.equal(minutesLeft(NOW + MIN, NOW), 1);
  assert.equal(minutesLeft(NOW + MIN - 1, NOW), null);
  assert.equal(minutesLeft(NOW, NOW), null);
  assert.equal(minutesLeft(NOW - 5, NOW), null);
});

test("the next tick is when the line would say something else, or when it goes", () => {
  for (const left of [15 * MIN, 14 * MIN + 1, 2 * MIN + 5000, 2 * MIN, MIN + 1, MIN, MIN - 1, 1000, 0]) {
    const until = NOW + left;
    const wait = nextTick(until, NOW);
    // Under a minute the line says the same until the message goes.
    if (left < MIN) {
      assert.equal(wait, left);
      continue;
    }
    assert.ok(wait > 0, `wait for ${left}`);
    assert.equal(minutesLeft(until, NOW + wait - 1), minutesLeft(until, NOW), `same until then (${left})`);
    assert.notEqual(minutesLeft(until, NOW + wait), minutesLeft(until, NOW), `changes then (${left})`);
  }
  assert.equal(nextTick(NOW - 10, NOW), 0);
});

test("merged puts the kept ones among the messages by time, after ties, and keeps the array without any", () => {
  const items: M[] = [
    { id: "1", at: NOW - 3000 },
    { id: "2", at: NOW - 2000 },
    { id: "3", at: NOW - 1000 },
  ];
  const time = (m: M) => m.at;
  assert.equal(merged(items, [], time), items);
  const list = [kept("e0", NOW - 5000, NOW + MIN), kept("e2", NOW - 2000, NOW + MIN), kept("e9", NOW, NOW + MIN)];
  assert.deepEqual(
    merged(items, list, time).map((m) => m.id),
    ["e0", "1", "2", "e2", "3", "e9"],
  );
  assert.deepEqual(
    merged([], list, time).map((m) => m.id),
    ["e0", "e2", "e9"],
  );
});
