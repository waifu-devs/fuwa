import assert from "node:assert/strict";
import { test } from "node:test";
import { DEFAULT_SHARE, SHARE_FPS, SHARE_HEIGHTS, screenEncodings, shareBox, shareMbps, shareQuality } from "./screen-share.ts";

test("the default is what sharing always sent", () => {
  assert.deepEqual(screenEncodings(DEFAULT_SHARE), [
    { rid: "l", scaleResolutionDownBy: 4, maxBitrate: 200_000, maxFramerate: 5 },
    { rid: "m", scaleResolutionDownBy: 2, maxBitrate: 700_000, maxFramerate: 15 },
    { rid: "h", scaleResolutionDownBy: 1, maxBitrate: 2_500_000, maxFramerate: 30 },
  ]);
});

test("every choice fits what the media server passes on (8 Mbit/s for a screen)", () => {
  for (const height of SHARE_HEIGHTS) {
    for (const fps of SHARE_FPS) {
      const total = screenEncodings({ height, fps }).reduce((sum, e) => sum + (e.maxBitrate ?? 0), 0);
      assert.ok(total <= 7_500_000, `${height}p${fps}: ${total}`);
      assert.equal(screenEncodings({ height, fps })[2].maxFramerate, fps);
    }
  }
});

test("more pixels or frames never get fewer bits", () => {
  const top = (height: (typeof SHARE_HEIGHTS)[number], fps: (typeof SHARE_FPS)[number]) => screenEncodings({ height, fps })[2].maxBitrate!;
  for (const fps of SHARE_FPS) assert.ok(top(720, fps) < top(1080, fps) && top(1080, fps) < top(1440, fps));
  for (const height of SHARE_HEIGHTS) assert.ok(top(height, 15) < top(height, 30) && top(height, 30) < top(height, 60));
});

test("sizes, upload and saved choices", () => {
  assert.deepEqual(shareBox(1440), { width: 2560, height: 1440 });
  assert.deepEqual(shareBox(720), { width: 1280, height: 720 });
  assert.equal(shareMbps(DEFAULT_SHARE), 3.4);
  assert.deepEqual(shareQuality(1440, 60), { height: 1440, fps: 60 });
  assert.deepEqual(shareQuality(4320, "fast"), DEFAULT_SHARE);
});
