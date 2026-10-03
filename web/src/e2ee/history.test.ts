import assert from "node:assert/strict";
import { test } from "node:test";
import { accept, type Candidate, type Context, type Logged } from "./history.ts";

const message = (senderId: string, deviceId: string, deleted = false): Logged => ({ kind: "message", senderId, deviceId, deleted });

function context(log: [number, Logged][], joined = 100): Context {
  return {
    joined,
    allowed: ["aoi", "mika", "juan"],
    devices: new Set(["aoi/a1", "mika/m1", "mika/m2", "juan/j1"]),
    log: new Map(log),
  };
}

test("takes entries signed by their sender's own device, at their place in the log", () => {
  const ctx = context([
    [3, message("aoi", "a1")],
    [4, message("mika", "m1")],
  ]);
  const entries: Candidate[] = [
    { seq: 3, senderId: "aoi", deviceId: "a1", kind: "text" },
    { seq: 4, senderId: "mika", deviceId: "m1", kind: "text" },
    { seq: 4, senderId: "mika", deviceId: "m2", kind: "edit", target: 4 },
  ];
  assert.deepEqual(accept(entries, ctx), [0, 1, 2]);
});

test("drops an entry signed with the sharer's own key that claims another member sent it", () => {
  const ctx = context([[3, message("aoi", "a1")]]);
  // Juan signs with a throwaway key and names Aoi as the sender.
  const forged: Candidate = { seq: 3, senderId: "aoi", deviceId: "throwaway", kind: "text" };
  assert.deepEqual(accept([forged], ctx), []);
  // Or with his own device: still not Aoi's.
  assert.deepEqual(accept([{ ...forged, deviceId: "j1" }], ctx), []);
});

test("drops entries from a device its owner removed", () => {
  const ctx = context([[3, message("aoi", "a-old")]]);
  assert.deepEqual(accept([{ seq: 3, senderId: "aoi", deviceId: "a-old", kind: "text" }], ctx), []);
});

test("a sharer can't move, repeat or bring back messages", () => {
  const ctx = context([
    [3, message("aoi", "a1")],
    [4, message("mika", "m1")],
    [5, message("aoi", "a1", true)],
    [6, { kind: "other", senderId: "aoi", deviceId: "a1", deleted: false }],
  ]);
  const aoi = (seq: number): Candidate => ({ seq, senderId: "aoi", deviceId: "a1", kind: "text" });
  assert.deepEqual(accept([aoi(4)], ctx), [], "moved onto someone else's message");
  assert.deepEqual(accept([aoi(3), aoi(3)], ctx), [0], "repeated");
  assert.deepEqual(accept([aoi(5)], ctx), [], "deleted");
  assert.deepEqual(accept([aoi(6)], ctx), [], "not a message");
  assert.deepEqual(accept([aoi(7)], ctx), [], "not in the log");
  assert.deepEqual(accept([{ seq: 3, senderId: "aoi", deviceId: "a1", kind: "edit", target: 3 }], ctx), [], "edit of nothing taken");
  assert.deepEqual(accept([aoi(3), { seq: 3, senderId: "aoi", deviceId: "a1", kind: "edit", target: 9 }], ctx), [0], "edit pointing elsewhere");
});

test("takes only what came before joining, and after sharing was turned on", () => {
  const ctx = context(
    [
      [3, message("aoi", "a1")],
      [5, { kind: "settings", senderId: "juan", deviceId: "j1", deleted: false, on: true }],
      [6, message("aoi", "a1")],
      [12, message("aoi", "a1")],
    ],
    10,
  );
  const aoi = (seq: number): Candidate => ({ seq, senderId: "aoi", deviceId: "a1", kind: "text" });
  assert.deepEqual(accept([aoi(3), aoi(6), aoi(12)], ctx), [1]);
});

test("takes nothing when sharing was last turned off", () => {
  const ctx = context([
    [2, { kind: "settings", senderId: "juan", deviceId: "j1", deleted: false, on: false }],
    [3, message("aoi", "a1")],
  ]);
  assert.deepEqual(accept([{ seq: 3, senderId: "aoi", deviceId: "a1", kind: "text" }], ctx), []);
});

test("drops senders no longer in the channel", () => {
  const ctx = { ...context([[3, message("rin", "r1")]]), devices: new Set(["rin/r1"]) };
  assert.deepEqual(accept([{ seq: 3, senderId: "rin", deviceId: "r1", kind: "text" }], ctx), []);
});
