import assert from "node:assert/strict";
import { test } from "node:test";
import type { ReadMark } from "@/gen/fuwa/v1/account_pb";
import type { InstanceState } from "./store";
import { byMessage, newestMark, readAlready, withReadMarks } from "./read-marks.ts";

const mark = (place: string, messageId: string, sequence = 0n, atMs = 0n) => ({ place, messageId, sequence, atMs }) as ReadMark;

const instance = (extra: Partial<InstanceState> = {}) =>
  ({ unread: {}, threadUnread: {}, readMarks: {}, ...extra }) as unknown as InstanceState;

test("a mark from another device clears a channel's and a thread's unread counts", () => {
  const inst = instance({ unread: { general: 3, random: 1 }, threadUnread: { t1: 2 } });
  const next = withReadMarks(inst, [mark("general", "m5"), mark("t1", "m9")]);
  assert.deepEqual(next.unread, { random: 1 });
  assert.deepEqual(next.threadUnread, {});
  assert.deepEqual(next.readMarks, { general: "m5", t1: "m9" });
  assert.ok(readAlready(next, "general", "m5"));
  assert.ok(!readAlready(next, "general", "m6"));
});

test("marks only move forward, and conversations' are left to the encryption engine", () => {
  const inst = instance({ readMarks: { general: "m5" }, unread: { dm: 2 } });
  const next = withReadMarks(inst, [mark("general", "m3"), mark("dm", "", 7n)]);
  assert.deepEqual(next.readMarks, { general: "m5" });
  assert.deepEqual(next.unread, { dm: 2 });
  assert.equal(withReadMarks(inst, []), inst);
  assert.ok(!byMessage(mark("dm", "", 7n)));
});

test("a connection asks for marks from the newest it heard", () => {
  assert.equal(newestMark([mark("a", "m1", 0n, 5n), mark("b", "m2", 0n, 9n)], 7n), 9n);
  assert.equal(newestMark([], 7n), 7n);
});
