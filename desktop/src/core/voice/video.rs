//! Cameras and shared screens in a call, between the connection and the
//! window (the web's calls/video.ts). Each is a feed: a camera's is its
//! person's account id, a shared screen's that and "-screen", as the media
//! part names its stream.
//!
//! Each feed that comes in gets a thread of its own that decodes it and
//! leaves only its latest picture for the window: a picture the window
//! hasn't taken yet is written over (its buffer reused), so nothing queues,
//! and a frame that can't wait is dropped and a keyframe asked for.
//!
//! The window says how tall it shows each feed, per window, and the call
//! asks the media part for the size that fits ("layers"), or none at all
//! for feeds nobody sees.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};

use parking_lot::Mutex;
use tokio::sync::watch;

use super::vp8::{Decoder, Picture, is_keyframe};

const SCREEN: &str = "-screen";

/// Frames a feed's decoder may have waiting before more are dropped.
const QUEUE: usize = 8;

/// The feed of someone's camera, or of their shared screen.
pub fn feed_of(user_id: &str, screen: bool) -> String {
    if screen { format!("{user_id}{SCREEN}") } else { user_id.to_owned() }
}

/// Whose a feed is.
pub fn owner_of(feed: &str) -> &str {
    feed.strip_suffix(SCREEN).unwrap_or(feed)
}

pub fn is_screen(feed: &str) -> bool {
    feed.ends_with(SCREEN)
}

/// The size of a camera that fits a picture this tall on screen, in device
/// pixels (the web's `layerFor`): a camera is 720 tall at most, a screen 1080.
pub fn layer_for(height: u32) -> &'static str {
    match height {
        0 => "off",
        1..=240 => "l",
        241..=480 => "m",
        _ => "h",
    }
}

#[derive(Default)]
struct Slot {
    picture: Option<Picture>,
    /// Counts the pictures put here.
    seq: u64,
}

/// The latest picture of every feed, and what the windows want of them.
pub struct Videos {
    slots: Mutex<HashMap<String, Slot>>,
    changes: watch::Sender<u64>,
    /// How tall each window shows each feed, by window.
    wants: Mutex<HashMap<u64, HashMap<String, u32>>>,
    wanted: watch::Sender<u64>,
}

impl Default for Videos {
    fn default() -> Self {
        Self {
            slots: Mutex::default(),
            changes: watch::channel(0).0,
            wants: Mutex::default(),
            wanted: watch::channel(0).0,
        }
    }
}

impl Videos {
    /// Bumps with every new picture.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Bumps when what the windows want changes.
    pub fn wanted_changes(&self) -> watch::Receiver<u64> {
        self.wanted.subscribe()
    }

    /// Puts a feed's new picture out for the window. One the window didn't
    /// take in time comes back in `picture`, so its buffer is used again.
    pub fn publish(&self, feed: &str, picture: &mut Picture) {
        {
            let mut slots = self.slots.lock();
            let slot = slots.entry(feed.to_owned()).or_default();
            if let Some(old) = slot.picture.replace(std::mem::take(picture)) {
                *picture = old;
            }
            slot.seq += 1;
        }
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// The feed's newest picture, if one came since the last time it was taken.
    pub fn take(&self, feed: &str) -> Option<Picture> {
        self.slots.lock().get_mut(feed).and_then(|s| s.picture.take())
    }

    /// How many pictures a feed has had: 0 while none came.
    pub fn seq(&self, feed: &str) -> u64 {
        self.slots.lock().get(feed).map_or(0, |s| s.seq)
    }

    /// A feed ended: it shows nothing until it's back.
    pub fn forget(&self, feed: &str) {
        if self.slots.lock().remove(feed).is_some() {
            self.changes.send_modify(|v| *v = v.wrapping_add(1));
        }
    }

    /// Every feed but the ones `keep` names ends.
    pub fn forget_all_but(&self, keep: impl Fn(&str) -> bool) {
        self.slots.lock().retain(|feed, _| keep(feed));
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// How tall `viewer` (a window) shows each feed now, in device pixels;
    /// feeds it doesn't show are left out.
    pub fn want(&self, viewer: u64, feeds: HashMap<String, u32>) {
        let mut wants = self.wants.lock();
        let before = wants.get(&viewer);
        if before == Some(&feeds) || (before.is_none() && feeds.is_empty()) {
            return;
        }
        if feeds.is_empty() {
            wants.remove(&viewer);
        } else {
            wants.insert(viewer, feeds);
        }
        drop(wants);
        self.wanted.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// How tall the biggest of the windows shows a feed: 0 when none does.
    pub fn wanted(&self, feed: &str) -> u32 {
        self.wants.lock().values().filter_map(|w| w.get(feed)).copied().max().unwrap_or(0)
    }
}

/// One feed coming in: its decoder, on a thread of its own.
pub struct Watcher {
    frames: SyncSender<Vec<u8>>,
    /// A frame was dropped or lost on the way: wait for a keyframe.
    lost: Arc<AtomicBool>,
    /// The decoder is waiting for a keyframe and wants one asked for.
    waiting: Arc<AtomicBool>,
}

impl Watcher {
    pub fn start(feed: String, videos: Arc<Videos>) -> Option<Self> {
        let (frames, incoming) = sync_channel::<Vec<u8>>(QUEUE);
        let lost = Arc::new(AtomicBool::new(false));
        let waiting = Arc::new(AtomicBool::new(true));
        let (l, w) = (lost.clone(), waiting.clone());
        std::thread::Builder::new()
            .name("fuwa-video-in".into())
            .spawn(move || {
                let Ok(mut decoder) = Decoder::new() else { return };
                let mut picture = Picture::default();
                let mut keyless = true;
                while let Ok(frame) = incoming.recv() {
                    if l.swap(false, Ordering::Relaxed) {
                        keyless = true;
                    }
                    if keyless && !is_keyframe(&frame) {
                        w.store(true, Ordering::Relaxed);
                        continue;
                    }
                    match decoder.decode(&frame, &mut picture.bgra) {
                        Some((width, height)) => {
                            keyless = false;
                            w.store(false, Ordering::Relaxed);
                            picture.width = width;
                            picture.height = height;
                            videos.publish(&feed, &mut picture);
                        }
                        None => {
                            keyless = true;
                            w.store(true, Ordering::Relaxed);
                        }
                    }
                }
            })
            .ok()?;
        Some(Self { frames, lost, waiting })
    }

    /// Hands a whole frame to the decoder; false when it had to be dropped.
    pub fn give(&self, frame: Vec<u8>, contiguous: bool) -> bool {
        if !contiguous {
            self.lost.store(true, Ordering::Relaxed);
        }
        match self.frames.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.lost.store(true, Ordering::Relaxed);
                self.waiting.store(true, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Whether it's waiting for a keyframe.
    pub fn waiting(&self) -> bool {
        self.waiting.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeds_name_their_person_and_screen() {
        assert_eq!(feed_of("u1", true), "u1-screen");
        assert_eq!(owner_of("u1-screen"), "u1");
        assert_eq!(owner_of("u1"), "u1");
        assert!(is_screen("u1-screen") && !is_screen("u1"));
        assert_eq!([0, 180, 240, 241, 480, 720].map(layer_for), ["off", "l", "l", "m", "m", "h"]);
    }

    #[test]
    fn only_the_latest_picture_waits_and_its_buffer_comes_back() {
        let videos = Videos::default();
        let mut picture = Picture { width: 2, height: 1, bgra: vec![1; 8] };
        videos.publish("a", &mut picture);
        assert!(picture.bgra.is_empty(), "nothing to give back yet");
        let mut next = Picture { width: 2, height: 1, bgra: vec![2; 8] };
        videos.publish("a", &mut next);
        assert_eq!(next.bgra, vec![1; 8], "the one not taken comes back to be written over");
        assert_eq!(videos.take("a").unwrap().bgra, vec![2; 8]);
        assert!(videos.take("a").is_none());
        assert_eq!(videos.seq("a"), 2);

        videos.want(1, HashMap::from([("a".into(), 300)]));
        videos.want(2, HashMap::from([("a".into(), 700)]));
        assert_eq!(videos.wanted("a"), 700);
        videos.want(2, HashMap::new());
        assert_eq!(videos.wanted("a"), 300);
        assert_eq!(videos.wanted("b"), 0);
    }
}
