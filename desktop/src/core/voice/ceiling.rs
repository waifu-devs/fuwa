//! How sharp and smooth your camera goes out in calls (docs/calls.md): the
//! best it gives, up to 1080p at 60 frames a second, under the lowest of
//! three ceilings: yours (Voice & video settings), the instance's and, in a
//! server's voice channel, the server's. The web's `lib/camera-quality.ts`,
//! number for number. Pure.

use super::vp8::Size;

/// The heights and frame rates people pick from in their settings; 0 is "Best".
pub const HEIGHTS: [u32; 5] = [0, 1080, 720, 480, 360];
pub const FRAME_RATES: [u32; 4] = [0, 60, 30, 15];
/// The ones admins pick from for an instance or a server; 0 is no ceiling.
pub const CEILING_HEIGHTS: [u32; 5] = [0, 1080, 720, 480, 360];
pub const CEILING_FRAME_RATES: [u32; 5] = [0, 60, 30, 24, 15];

/// The most a camera sends: its height in pixels and frames a second. Zero
/// in either means no ceiling there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ceiling {
    pub height: u32,
    pub fps: u32,
}

/// The best a camera goes out at, with nothing lower asked: 1080p at 60
/// (the web's `BEST`).
pub const BEST: Ceiling = Ceiling { height: 1080, fps: 60 };

impl Ceiling {
    /// No ceiling at all.
    pub const NONE: Self = Self { height: 0, fps: 0 };

    /// The lowest of each, among those set: your own choice (0 is the best),
    /// the instance's and, in a server's voice channel, the server's (the
    /// web's `cameraCeiling`).
    /// Never above [`BEST`], whatever an instance allows.
    pub fn lowest(mine: Ceiling, instance: Ceiling, server: Ceiling) -> Ceiling {
        let low = |best: u32, all: [u32; 3]| all.into_iter().filter(|&n| n > 0).fold(best, u32::min);
        Ceiling {
            height: low(BEST.height, [mine.height, instance.height, server.height]),
            fps: low(BEST.fps, [mine.fps, instance.fps, server.fps]),
        }
    }

    /// The size asked of the camera: 16:9 at this height.
    pub fn ask(&self) -> (u32, u32) {
        ((f64::from(self.height) * 16.0 / 9.0).round() as u32, self.height)
    }

    /// A camera's three sizes, smallest first, as the web's `cameraEncodings`:
    /// the full size at the ceiling's rate, half at up to 30, a quarter at up
    /// to 15. Their bitrates come from the pixels each has ([`bitrate_for`]).
    pub fn sizes(&self) -> [Size; 3] {
        let fps = self.fps.max(1);
        let size = |rid, divide, fps: u32| Size { rid, divide, bitrate: 0, fps };
        [size("l", 4, fps.min(15)), size("m", 2, fps.min(30)), size("h", 1, fps)]
    }
}

/// The bitrate for a camera's picture of `pixels` at `fps`, in bits a
/// second (the web's `cameraBitrate`): about 1.5 Mbit/s for 720p at 30,
/// 5.2 for 1080p at 60, never under 150 kbit/s.
pub fn bitrate_for(pixels: u32, fps: u32) -> u32 {
    let bits = f64::from(pixels) * 1.65 * (f64::from(fps) / 30.0).powf(0.6);
    (bits.round() as u32).max(150_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_ceilings_and_bitrates_match_the_web() {
        let none = Ceiling::NONE;
        assert_eq!(Ceiling::lowest(none, none, none), BEST);
        let mine = Ceiling { height: 720, fps: 0 };
        let instance = Ceiling { height: 0, fps: 30 };
        let server = Ceiling { height: 480, fps: 0 };
        assert_eq!(Ceiling::lowest(mine, instance, none), Ceiling { height: 720, fps: 30 });
        assert_eq!(Ceiling::lowest(mine, instance, server), Ceiling { height: 480, fps: 30 });
        // A ceiling above the best doesn't raise it.
        assert_eq!(Ceiling::lowest(none, Ceiling { height: 2160, fps: 120 }, none), BEST);
        assert_eq!(BEST.ask(), (1920, 1080));
        assert_eq!(Ceiling { height: 720, fps: 30 }.ask(), (1280, 720));
        assert_eq!(Ceiling { height: 480, fps: 30 }.ask(), (853, 480));

        let fps = |c: Ceiling| c.sizes().map(|s| (s.rid, s.divide, s.fps));
        assert_eq!(fps(BEST), [("l", 4, 15), ("m", 2, 30), ("h", 1, 60)]);
        assert_eq!(fps(Ceiling { height: 720, fps: 15 }), [("l", 4, 15), ("m", 2, 15), ("h", 1, 15)]);
        assert_eq!(fps(Ceiling { height: 720, fps: 24 }), [("l", 4, 15), ("m", 2, 24), ("h", 1, 24)]);

        assert_eq!(bitrate_for(1280 * 720, 30), 1_520_640);
        assert_eq!(bitrate_for(1920 * 1080, 60), 5_185_933);
        assert_eq!(bitrate_for(320 * 180, 15), 150_000);
        assert_eq!(bitrate_for(640 * 360, 30), 380_160);
    }
}
