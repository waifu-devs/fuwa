import assert from "node:assert/strict";
import { test } from "node:test";
import {
  channelPlace,
  dmPlace,
  expire,
  inChannel,
  nextExpiry,
  SEND_EVERY_MS,
  started,
  stopped,
  stoppedWhere,
  throttle,
  TYPING_MS,
  typers,
  typingLabel,
  type Typing,
} from "./typing.ts";

const NOW = 1_800_000_000_000;
const ME = "me";

test("places: a channel, a thread's reply box in it, a conversation", () => {
  assert.equal(channelPlace("c1"), "c1");
  assert.equal(channelPlace("c1", "t1"), "c1#t1");
  assert.equal(dmPlace("d1"), "dm:d1");
  assert.ok(inChannel("c1", "c1"));
  assert.ok(inChannel("c1#t1", "c1"));
  assert.ok(!inChannel("c10", "c1"));
  assert.ok(!inChannel("dm:c1", "c1"));
});

test("a signal shows someone for ten seconds, and another one keeps them", () => {
  let s: Typing = started({}, "c1", "ana", ME, NOW);
  assert.deepEqual(typers(s, "c1", NOW), ["ana"]);
  assert.deepEqual(typers(s, "c1", NOW + TYPING_MS - 1), ["ana"]);
  assert.deepEqual(typers(s, "c1", NOW + TYPING_MS), []);
  assert.equal(nextExpiry(s), NOW + TYPING_MS);

  s = started(s, "c1", "ana", ME, NOW + 6_000);
  assert.deepEqual(typers(s, "c1", NOW + TYPING_MS + 1), ["ana"]);
  assert.equal(nextExpiry(s), NOW + 6_000 + TYPING_MS);
  assert.deepEqual(typers(s, "c2", NOW), []);
});

test("expiry drops only whoever ran out, and changes nothing otherwise", () => {
  let s: Typing = started({}, "c1", "ana", ME, NOW);
  s = started(s, "c1", "ben", ME, NOW + 4_000);
  s = started(s, "dm:d1", "cy", ME, NOW);
  assert.equal(expire(s, NOW + 1_000), s);

  const later = expire(s, NOW + TYPING_MS);
  assert.deepEqual(later, { c1: { ben: NOW + 4_000 + TYPING_MS } });
  // A place nobody's left in is gone, and one still the same keeps its object.
  const s2 = started(s, "c2", "dee", ME, NOW + 9_000);
  assert.equal(expire(s2, NOW + TYPING_MS).c2, s2.c2);
  assert.deepEqual(expire(later, NOW + 4_000 + TYPING_MS), {});
  assert.equal(nextExpiry({}), null);
});

test("a message from someone ends their typing there, and only there", () => {
  let s: Typing = started({}, "c1", "ana", ME, NOW);
  s = started(s, "c1#t1", "ana", ME, NOW);
  s = started(s, "c1", "ben", ME, NOW);
  const after = stopped(s, "c1", "ana");
  assert.deepEqual(typers(after, "c1", NOW), ["ben"]);
  assert.deepEqual(typers(after, "c1#t1", NOW), ["ana"]);
  // Nobody there: nothing changes.
  assert.equal(stopped(s, "c1", "cy"), s);
  assert.equal(stopped(s, "c9", "ana"), s);
  // The last one out takes the place with them.
  assert.deepEqual(Object.keys(stopped(after, "c1#t1", "ana")), ["c1"]);
});

test("a secure channel's message ends someone's typing in the channel and its threads", () => {
  let s: Typing = started({}, "c1", "ana", ME, NOW);
  s = started(s, "c1#4", "ana", ME, NOW);
  s = started(s, "c2", "ana", ME, NOW);
  const after = stoppedWhere(s, (p) => inChannel(p, "c1"), "ana");
  assert.deepEqual(after, { c2: { ana: NOW + TYPING_MS } });
});

test("you're never shown as typing to yourself", () => {
  const s: Typing = {};
  assert.equal(started(s, "c1", ME, ME, NOW), s);
  assert.equal(started(s, "c1", "", ME, NOW), s);
  assert.deepEqual(typers(started(s, "c1", "ana", undefined, NOW), "c1", NOW), ["ana"]);
});

test("people keep the order they started typing in", () => {
  let s: Typing = started({}, "c1", "ana", ME, NOW);
  s = started(s, "c1", "ben", ME, NOW + 1);
  s = started(s, "c1", "ana", ME, NOW + 2);
  assert.deepEqual(typers(s, "c1", NOW + 3), ["ana", "ben"]);
});

test("the line names one, two or three people, then says several", () => {
  assert.equal(typingLabel([]), null);
  assert.deepEqual(typingLabel(["Ana"]), { kind: "one", names: ["Ana"] });
  assert.deepEqual(typingLabel(["Ana", "Ben"]), { kind: "two", names: ["Ana", "Ben"] });
  assert.deepEqual(typingLabel(["Ana", "Ben", "Cy"]), { kind: "three", names: ["Ana", "Ben", "Cy"] });
  assert.deepEqual(typingLabel(["Ana", "Ben", "Cy", "Dee"]), { kind: "several", names: [] });
  assert.deepEqual(typingLabel(["a", "b", "c", "d", "e", "f"]), { kind: "several", names: [] });
});

test("signals go out at most every six seconds per place, and at once after sending", () => {
  const t = throttle();
  assert.ok(t.take("c1", NOW));
  assert.ok(!t.take("c1", NOW + 1));
  assert.ok(!t.take("c1", NOW + SEND_EVERY_MS - 1));
  // Another place has its own clock.
  assert.ok(t.take("c1#t1", NOW + 1));
  assert.ok(t.take("c1", NOW + SEND_EVERY_MS));
  assert.ok(!t.take("c1", NOW + SEND_EVERY_MS + 1));
  // Sending a message there starts it over.
  t.reset("c1");
  assert.ok(t.take("c1", NOW + SEND_EVERY_MS + 2));
  assert.ok(!t.take("c1", NOW + SEND_EVERY_MS + 3));
  t.reset("never");
});
