//! Small files made with ffmpeg (`tests/media`): a second of 64x48 at 20
//! frames a second, with a 440 Hz tone where there's sound.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;

fn file(bytes: &[u8]) -> Fetched {
    Arc::from(bytes)
}

const MP4: &[u8] = include_bytes!("../../../tests/media/clip.mp4");
const WEBM: &[u8] = include_bytes!("../../../tests/media/clip.webm");
const SILENT: &[u8] = include_bytes!("../../../tests/media/silent.webm");
const MP3: &[u8] = include_bytes!("../../../tests/media/tone.mp3");
const AV1: &[u8] = include_bytes!("../../../tests/media/av1.webm");

/// Every frame a video's picture gives, with when each is shown.
fn frames(bytes: &[u8], content_type: &str, name: &str, from: Option<f64>) -> Vec<(f64, u32, u32)> {
    let source = video::open(&file(bytes), content_type, name).unwrap().unwrap();
    video::tests::decode_all(source, from)
}

fn sound_length_ms(bytes: &[u8], content_type: &str, name: &str) -> (u64, f64) {
    let source = sound::open(&file(bytes), content_type, name).unwrap();
    sound::tests::play_through(source)
}

#[test]
fn h264_in_mp4_decodes_every_frame_in_order() {
    let got = frames(MP4, "video/mp4", "clip.mp4", None);
    assert_eq!(got.len(), 20);
    assert!(got.iter().all(|(_, w, h)| (*w, *h) == (64, 48)));
    // B-frames come out of the decoder in the order they're shown.
    assert!(got.windows(2).all(|w| w[0].0 < w[1].0), "{got:?}");
    assert!(got[0].0.abs() < 1.0, "the first frame shows at the start: {got:?}");
}

#[test]
fn vp9_in_webm_decodes_every_frame() {
    let got = frames(WEBM, "video/webm", "clip.webm", None);
    assert_eq!(got.len(), 20);
    assert!(got.iter().all(|(_, w, h)| (*w, *h) == (64, 48)));
}

#[test]
fn jumping_lands_on_the_frame_there() {
    for (bytes, kind, name) in [(MP4, "video/mp4", "clip.mp4"), (WEBM, "video/webm", "clip.webm")] {
        let got = frames(bytes, kind, name, Some(620.0));
        let first = got.first().unwrap().0;
        assert!((580.0..=670.0).contains(&first), "{name}: {got:?}");
        assert!(got.len() <= 9, "{name} plays on from there: {got:?}");
    }
}

#[test]
fn a_browser_recording_without_a_length_is_measured() {
    let source = video::open(&file(SILENT), "video/webm", "silent.webm").unwrap().unwrap();
    assert!((900..=1000).contains(&source.duration_ms()), "{}", source.duration_ms());
    assert!(sound::open(&file(SILENT), "video/webm", "silent.webm").is_none());
}

#[test]
fn sound_plays_through_to_its_length() {
    for (bytes, kind, name) in
        [(MP4, "video/mp4", "clip.mp4"), (WEBM, "video/webm", "clip.webm"), (MP3, "audio/mpeg", "tone.mp3")]
    {
        let (said, played) = sound_length_ms(bytes, kind, name);
        assert!((950..=1100).contains(&said), "{name} says {said} ms");
        assert!((900.0..=1150.0).contains(&played), "{name} played {played} ms");
    }
}

#[test]
fn a_sound_file_has_no_picture() {
    assert!(matches!(video::open(&file(MP3), "audio/mpeg", "tone.mp3"), Ok(None)));
    let player = Player::open(file(MP3), "audio/mpeg", "tone.mp3").unwrap();
    assert!(!player.has_picture());
}

#[test]
fn av1_and_unknown_files_dont_play() {
    assert_eq!(Player::open(file(AV1), "video/webm", "av1.webm").err(), Some(Unplayable::Kind));
    assert_eq!(Player::open(file(b"not a video at all"), "video/mp4", "x.mp4").err(), Some(Unplayable::Kind));
    assert_eq!(Player::open(file(b"neither"), "application/zip", "x.zip").err(), Some(Unplayable::Kind));
}

#[test]
fn a_video_plays_to_the_end_and_stops_there() {
    let player = Player::open(file(SILENT), "video/webm", "silent.webm").unwrap();
    assert!(player.is_paused());
    player.play();
    let started = Instant::now();
    let mut pictures = 0;
    while !player.has_ended() && started.elapsed() < Duration::from_secs(5) {
        if let Some(p) = player.take_picture() {
            assert_eq!((p.width, p.height), (64, 48));
            pictures += 1;
            player.give_back(p.bgra);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(player.has_ended());
    assert!(player.is_paused());
    assert!(pictures >= 10, "{pictures} pictures");
    let (at, length) = player.position_ms();
    assert_eq!(at, length);
    // Playing again starts over.
    player.play();
    std::thread::sleep(Duration::from_millis(100));
    assert!(player.position_ms().0 < 500);
}

#[test]
fn pausing_holds_the_place() {
    let player = Player::open(file(SILENT), "video/webm", "silent.webm").unwrap();
    player.play();
    std::thread::sleep(Duration::from_millis(200));
    player.pause();
    let (at, _) = player.position_ms();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(player.position_ms().0, at);
    player.seek(0.5);
    let (at, length) = player.position_ms();
    assert_eq!(at, length / 2);
}
