/*
 * How sharp and smooth your camera goes out in calls (docs/calls.md): the
 * best it gives, up to 1080p at 60 frames a second, under the lowest of
 * three ceilings: yours (Voice & video settings), the instance's and, in a
 * server's voice channel, the server's. The desktop's
 * `core/voice/ceiling.rs` is the same, number for number. Pure.
 */

/** A ceiling on cameras: the tallest picture in pixels and the most frames a second; 0 for none. */
export type Ceiling = { height: number; fps: number };

/** What an app sends with no ceiling at all. */
export const BEST: Ceiling = { height: 1080, fps: 60 };

/** The heights and frame rates people pick from; 0 is "Best". */
export const HEIGHTS = [0, 1080, 720, 480, 360] as const;
export const FRAME_RATES = [0, 60, 30, 15] as const;
/** The ones admins pick from for an instance or a server; 0 is no ceiling. */
export const CEILING_HEIGHTS = [0, 1080, 720, 480, 360] as const;
export const CEILING_FRAME_RATES = [0, 60, 30, 24, 15] as const;

/** The lowest of the ceilings, where none is BEST. */
export function ceilingOf(...ceilings: (Partial<Ceiling> | null | undefined)[]): Ceiling {
  const low = (key: keyof Ceiling) => Math.min(BEST[key], ...ceilings.map((c) => c?.[key] || Infinity));
  return { height: low("height"), fps: low("fps") };
}

/** Bits a second for a picture of this many pixels at this frame rate: 720p30 about 1.5 Mbit/s, 1080p60 about 5.2. */
export function bitrateFor(pixels: number, fps: number): number {
  return Math.max(150_000, Math.round(pixels * 1.65 * Math.pow(fps / 30, 0.6)));
}

/**
 * The three sizes a camera goes out in, smallest first (as browsers want
 * them): a quarter at up to 15 frames a second, half at up to 30, and all of
 * it at the ceiling's, each with a ceiling on its bitrate from the pixels it has.
 */
export function cameraEncodings(width: number, height: number, fps: number): RTCRtpEncodingParameters[] {
  const layer = (rid: string, scale: number, most: number) => {
    const rate = Math.min(fps, most);
    return { rid, scaleResolutionDownBy: scale, maxBitrate: bitrateFor(Math.round(width / scale) * Math.round(height / scale), rate), maxFramerate: rate };
  };
  return [layer("l", 4, 15), layer("m", 2, 30), layer("h", 1, Infinity)];
}

/** The width that goes with a height, for a 16:9 camera. */
export const widthFor = (height: number) => Math.round((height * 16) / 9);
