//! The main window's pictures (avatars, server icons, emoji, banners), kept
//! within a budget instead of forever.
//!
//! GPUI keeps every picture it ever decoded unless an image cache says
//! otherwise, so a long session that scrolls through many channels and
//! member lists only ever grows. The cached parts of the window
//! (`panel.rs`) load their pictures through this cache, which keeps the ones
//! on screen and, past those, the most recently used up to [`BUDGET`].
//!
//! A cached part that isn't drawn again keeps showing what it painted last
//! time without asking for its pictures, so not being asked for doesn't mean
//! a picture is off screen. Only a frame in which every part is drawn anew
//! (after `window.refresh()`) says which pictures are on screen: the cache
//! asks for one when it's over budget (at most every [`EVERY`]), and lets go,
//! oldest first, only of pictures that frame didn't use.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, AppContext as _, Entity, ImageCache, ImageCacheError, ImageCacheItem, RenderImage, Resource, Window,
};

/// Decoded bytes kept for pictures that aren't on screen (and all of them, while under it).
const BUDGET: usize = 48 * 1024 * 1024;
/// How often at most every part is drawn anew to find out what's on screen.
const EVERY: Duration = Duration::from_secs(20);

struct Entry {
    item: ImageCacheItem,
    /// The window frame (`floats::frame`) that last asked for it.
    used: u64,
}

pub struct Images {
    entries: HashMap<Resource, Entry>,
    /// The frame drawn in full after asking, once it has been drawn.
    full: Option<u64>,
    /// Asked for a full frame, which comes next.
    asked: bool,
    last_ask: Option<Instant>,
}

impl Images {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let cache = cx.new(|_| Self { entries: HashMap::new(), full: None, asked: false, last_ask: None });
        cx.observe_release(&cache, |cache, cx| {
            for (_, entry) in cache.entries.drain() {
                if let Some(Ok(image)) = entry.item.get() {
                    cx.drop_image(image, None);
                }
            }
        })
        .detach();
        cache
    }

    fn bytes(image: &RenderImage) -> usize {
        (0..image.frame_count()).map(|i| image.as_bytes(i).map_or(0, <[u8]>::len)).sum()
    }

    fn total(&self) -> usize {
        self.entries.values().filter_map(|e| e.item.get()).filter_map(Result::ok).map(|i| Self::bytes(&i)).sum()
    }

    /// At the start of each of the window's frames (`frame` is `floats::frame`).
    pub fn frame(cache: &Entity<Self>, frame: u64, window: &mut Window, cx: &mut App) {
        let ask = cache.update(cx, |this, cx| {
            if this.asked {
                // This frame is drawn in full: what it asks for is what's on screen.
                this.asked = false;
                this.full = Some(frame);
                return false;
            }
            if let Some(full) = this.full.take() {
                this.trim(full, window, cx);
                return false;
            }
            let due = this.last_ask.is_none_or(|at| at.elapsed() >= EVERY);
            due && this.total() > BUDGET
        });
        if ask {
            let cache = cache.downgrade();
            window.defer(cx, move |window, cx| {
                let _ = cache.update(cx, |this, _| {
                    this.asked = true;
                    this.last_ask = Some(Instant::now());
                });
                window.refresh();
            });
        }
    }

    /// Lets go of the pictures the full frame `full` didn't use, oldest first,
    /// until the rest fit the budget.
    fn trim(&mut self, full: u64, window: &mut Window, cx: &mut App) {
        let mut total = self.total();
        if total <= BUDGET {
            return;
        }
        let mut gone: Vec<(u64, Resource)> =
            self.entries.iter().filter(|(_, e)| e.used < full).map(|(r, e)| (e.used, r.clone())).collect();
        gone.sort_by_key(|(used, _)| *used);
        let mut dropped = 0;
        for (_, resource) in gone {
            if total <= BUDGET {
                break;
            }
            let Some(entry) = self.entries.remove(&resource) else { continue };
            if let Some(Ok(image)) = entry.item.get() {
                total = total.saturating_sub(Self::bytes(&image));
                cx.drop_image(image, Some(window));
            }
            dropped += 1;
        }
        self.entries.shrink_to_fit();
        tracing::debug!(target: "fuwa_desktop::perf", "let go of {dropped} pictures, {} MB kept", total / (1024 * 1024));
    }
}

impl ImageCache for Images {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let frame = crate::ui::floats::frame();
        let entry = self
            .entries
            .entry(resource.clone())
            .or_insert_with(|| Entry { item: ImageCacheItem::new(resource, cx), used: frame });
        entry.used = frame;
        entry.item.use_image(window)
    }
}
