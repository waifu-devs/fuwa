import assert from "node:assert/strict";
import { test } from "node:test";
import { actualScale, clampView, FITTED, fitted, mostScale, panBy, stepIndex, toggleAt, wheelFactor, zoomAt } from "./zoom.ts";

// Run with `pnpm test` (node's own test runner; no extra dependencies).

const room = { width: 1000, height: 800 };
const big = { width: 4000, height: 2000 };
const small = { width: 200, height: 100 };

test("a picture is laid out fitted, never stretched", () => {
  assert.deepEqual(fitted(big, room), { width: 1000, height: 500 });
  assert.deepEqual(fitted(small, room), small);
  assert.equal(actualScale(big, room), 4);
  assert.equal(actualScale(small, room), 1);
  assert.equal(mostScale(big, room), 16);
  assert.equal(mostScale(small, room), 8);
});

test("zooming keeps the point under the pointer still", () => {
  const at = { x: 200, y: 100 };
  const view = zoomAt(FITTED, 2, at, big, room);
  assert.equal(view.scale, 2);
  // The point 200px right of the middle was 200/1 = 200px into the picture; at 2x it's 400px in, so the picture moves left by 200.
  assert.equal(view.x, -200);
  assert.equal(view.y, -100);
  // And back out returns to where it was.
  assert.deepEqual(zoomAt(view, 1, at, big, room), FITTED);
});

test("it never zooms past the limits or out past fitted", () => {
  assert.equal(zoomAt(FITTED, 1000, { x: 0, y: 0 }, big, room).scale, 16);
  assert.deepEqual(zoomAt(FITTED, 0.2, { x: 0, y: 0 }, big, room), FITTED);
});

test("a zoomed picture pans only as far as its edges", () => {
  const view = zoomAt(FITTED, 2, { x: 0, y: 0 }, big, room);
  // 2000x1000 in a 1000x800 room: 500 each way across, 100 each way down.
  assert.deepEqual(panBy(view, 9999, 9999, big, room), { scale: 2, x: 500, y: 100 });
  assert.deepEqual(panBy(view, -9999, -9999, big, room), { scale: 2, x: -500, y: -100 });
  // Fitted, it doesn't move at all.
  assert.deepEqual(panBy(FITTED, 50, 50, big, room), FITTED);
  assert.deepEqual(clampView({ scale: 1, x: 30, y: -30 }, big, room), FITTED);
});

test("a double click goes to 1:1 and back", () => {
  const view = toggleAt(FITTED, { x: 0, y: 0 }, big, room);
  assert.equal(view.scale, 4);
  assert.deepEqual(toggleAt(view, { x: 0, y: 0 }, big, room), FITTED);
  // A picture already at its own size goes to twice that.
  assert.equal(toggleAt(FITTED, { x: 0, y: 0 }, small, room).scale, 2);
});

test("the wheel zooms in going up and out going down, a pinch faster", () => {
  assert.ok(wheelFactor(-100, false) > 1);
  assert.ok(wheelFactor(100, false) < 1);
  assert.ok(wheelFactor(-10, true) > wheelFactor(-10, false));
  // One huge jump is held to a notch.
  assert.equal(wheelFactor(-5000, false), wheelFactor(-100, false));
});

test("stepping goes round the message's pictures", () => {
  assert.equal(stepIndex(0, 3, 1), 1);
  assert.equal(stepIndex(2, 3, 1), 0);
  assert.equal(stepIndex(0, 3, -1), 2);
  assert.equal(stepIndex(0, 0, 1), 0);
});
