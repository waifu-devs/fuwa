import assert from "node:assert/strict";
import { test } from "node:test";
import { bitrateFor, BEST, cameraEncodings, ceilingOf, widthFor } from "./camera-quality.ts";

test("the lowest ceiling wins, and none is the best", () => {
  assert.deepEqual(ceilingOf(), BEST);
  assert.deepEqual(ceilingOf({ height: 0, fps: 0 }, null, undefined), { height: 1080, fps: 60 });
  assert.deepEqual(ceilingOf({ height: 720, fps: 0 }, { height: 0, fps: 30 }), { height: 720, fps: 30 });
  assert.deepEqual(ceilingOf({ height: 2160, fps: 120 }), BEST, "never above the best");
  assert.deepEqual(ceilingOf({ height: 1080, fps: 60 }, { height: 480, fps: 15 }, { height: 720, fps: 24 }), { height: 480, fps: 15 });
});

test("bitrates follow the pixels and the frame rate, the desktop's numbers", () => {
  assert.equal(bitrateFor(1280 * 720, 30), 1_520_640);
  assert.equal(bitrateFor(1920 * 1080, 60), 5_185_933);
  assert.equal(bitrateFor(320 * 180, 15), 150_000, "a floor for tiny pictures");
  assert.equal(widthFor(1080), 1920);
  assert.equal(widthFor(720), 1280);
});

test("three sizes, smallest first, each at its own frame rate", () => {
  const [l, m, h] = cameraEncodings(1920, 1080, 60);
  assert.deepEqual([l!.rid, m!.rid, h!.rid], ["l", "m", "h"]);
  assert.deepEqual([l!.maxFramerate, m!.maxFramerate, h!.maxFramerate], [15, 30, 60]);
  assert.equal(h!.maxBitrate, bitrateFor(1920 * 1080, 60));
  assert.equal(m!.maxBitrate, bitrateFor(960 * 540, 30));
  const slow = cameraEncodings(1280, 720, 15);
  assert.deepEqual(
    slow.map((e) => e.maxFramerate),
    [15, 15, 15],
  );
  // Every size together stays well inside what the media part lets one camera send (2 MiB a second).
  const total = cameraEncodings(1920, 1080, 60).reduce((sum, e) => sum + e.maxBitrate!, 0);
  assert.ok(total / 8 < 1024 * 1024, `${total} bits a second`);
});
