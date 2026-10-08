/**
 * How a shared screen goes out: what to share first (a hint for the
 * browser's own picker), how sharp and how smooth. Pure, and the same
 * numbers as the desktop's `core/voice/vp8.rs` `screen_sizes`, so a share
 * looks alike from either app.
 *
 * Every choice stays within what the media server passes on for one
 * person's screen, all three sizes together.
 */

/** What the browser's picker opens on: a whole screen, one window, or a tab. */
export type ShareSurface = "monitor" | "window" | "browser";
export const SHARE_SURFACES: readonly ShareSurface[] = ["monitor", "window", "browser"];

/** How tall the full size is, at most (16:9 fits in it; other shapes keep their own). */
export type ShareHeight = 720 | 1080 | 1440;
export const SHARE_HEIGHTS: readonly ShareHeight[] = [720, 1080, 1440];

export type ShareFps = 15 | 30 | 60;
export const SHARE_FPS: readonly ShareFps[] = [15, 30, 60];

export type ShareQuality = { height: ShareHeight; fps: ShareFps };

/** What sharing used before there was a choice: 1080p at 30. */
export const DEFAULT_SHARE: ShareQuality = { height: 1080, fps: 30 };

/** The box the full size fits in. */
export const shareBox = (height: ShareHeight) => ({ width: Math.round((height * 16) / 9), height });

/** Bits a second for the full size: more pixels and more frames want more, text most of all. */
const TOP: Record<ShareHeight, Record<ShareFps, number>> = {
  720: { 15: 1_200_000, 30: 1_800_000, 60: 2_800_000 },
  1080: { 15: 1_800_000, 30: 2_500_000, 60: 4_000_000 },
  1440: { 15: 3_000_000, 30: 4_500_000, 60: 6_000_000 },
};

/** The half size gets a little more at 1440p, where it's 720p itself. */
const middle = (height: ShareHeight) => (height === 1440 ? 1_000_000 : 700_000);

/**
 * A shared screen's three sizes: a quarter for thumbnails, half, and full
 * for reading it, at the frame rate asked for.
 */
export function screenEncodings(q: ShareQuality): RTCRtpEncodingParameters[] {
  return [
    { rid: "l", scaleResolutionDownBy: 4, maxBitrate: 200_000, maxFramerate: 5 },
    { rid: "m", scaleResolutionDownBy: 2, maxBitrate: middle(q.height), maxFramerate: 15 },
    { rid: "h", scaleResolutionDownBy: 1, maxBitrate: TOP[q.height][q.fps], maxFramerate: q.fps },
  ];
}

/** About how much upload a share takes, in Mbit/s, every size together. */
export const shareMbps = (q: ShareQuality) => Math.round(screenEncodings(q).reduce((sum, e) => sum + (e.maxBitrate ?? 0), 0) / 100_000) / 10;

/** Smooth motion over sharp text, for games and video. */
export const wantsMotion = (q: ShareQuality) => q.fps >= 60;

/** A saved choice read back, anything unknown as the default. */
export function shareQuality(height: unknown, fps: unknown): ShareQuality {
  return {
    height: SHARE_HEIGHTS.includes(height as ShareHeight) ? (height as ShareHeight) : DEFAULT_SHARE.height,
    fps: SHARE_FPS.includes(fps as ShareFps) ? (fps as ShareFps) : DEFAULT_SHARE.fps,
  };
}
