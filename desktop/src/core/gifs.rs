//! GIFs, as the web app's `fuwa/gifs.ts`: searching the provider the
//! instance's admins set up (GIPHY or Klipy), which the instance asks itself
//! with nothing about who searched, and whose every picture comes back
//! through the instance. The GIFs each person saves are kept on the
//! instance; the ones sent lately are kept on this computer.

use serde::{Deserialize, Serialize};

use crate::core::api::Problem;
use crate::core::store::{self, upsert_message};
use crate::core::{Core, reports};
use crate::pb;
use crate::rpc;

/// GIFs sent lately kept on this computer, per instance.
pub const RECENT: usize = 30;
/// Results asked for at a time.
pub const PAGE: i32 = 24;
/// The longest search the instance takes.
pub const QUERY: usize = 100;

/// The biggest a GIF shows in a message.
const MAX_W: f32 = 320.0;
const MAX_H: f32 = 260.0;

/// A GIF sent lately, as it's kept in the settings file: sealed, so it can
/// go out again as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct KeptGif {
    pub url: String,
    pub width: i32,
    pub height: i32,
    pub title: String,
    pub provider: i32,
    pub seal: String,
}

impl KeptGif {
    pub fn of(gif: &pb::MessageGif) -> Self {
        Self {
            url: gif.url.clone(),
            width: gif.width,
            height: gif.height,
            title: gif.title.clone(),
            provider: gif.provider,
            seal: gif.seal.clone(),
        }
    }

    pub fn gif(&self) -> pb::MessageGif {
        pb::MessageGif {
            url: self.url.clone(),
            width: self.width,
            height: self.height,
            title: self.title.clone(),
            provider: self.provider,
            seal: self.seal.clone(),
        }
    }
}

fn missing() -> Problem {
    Problem::new(tonic::Code::NotFound, "That instance isn't here.")
}

/// Puts a GIF at the top of the ones sent lately, once, keeping [`RECENT`].
/// Ones without a seal couldn't be sent again, so they aren't kept.
pub fn remember(list: &mut Vec<KeptGif>, gif: &pb::MessageGif) {
    if gif.url.is_empty() || gif.seal.is_empty() {
        return;
    }
    list.retain(|g| g.url != gif.url);
    list.insert(0, KeptGif::of(gif));
    list.truncate(RECENT);
}

/// Who to credit for results, as their terms ask.
pub fn provider_name(provider: i32) -> &'static str {
    match pb::GifProvider::try_from(provider) {
        Ok(pb::GifProvider::Giphy) => "GIPHY",
        Ok(pb::GifProvider::Klipy) => "Klipy",
        _ => "",
    }
}

/// How big a GIF shows in a message: its own size, shrunk to fit, never
/// stretched, and never so small it can't be pointed at.
pub fn fit(width: i32, height: i32) -> (f32, f32) {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (MAX_W / w).min(MAX_H / h).min(1.0);
    ((w * scale).round().max(48.0), (h * scale).round().max(32.0))
}

/// Where each tile of a two-column masonry goes, from each one's height over
/// its width (a lead tile, if any, comes first): (x, y, width, height), and
/// how tall the whole is. Each tile goes in the shorter column, so nothing
/// moves when more come.
pub fn masonry(ratios: &[f32], width: f32, gap: f32, pad: f32) -> (Vec<(f32, f32, f32, f32)>, f32) {
    let column = ((width - pad * 2.0 - gap) / 2.0).max(60.0);
    let mut heights = [pad, pad];
    let mut placed = Vec::with_capacity(ratios.len());
    for &ratio in ratios {
        let c = if heights[0] <= heights[1] { 0 } else { 1 };
        let ratio = if ratio.is_finite() && ratio > 0.0 { ratio } else { 1.0 };
        let h = (column * ratio.clamp(0.45, 2.2)).round();
        placed.push((pad + c as f32 * (column + gap), heights[c], column, h));
        heights[c] += h + gap;
    }
    (placed, heights[0].max(heights[1]) + pad)
}

impl Core {
    /// Whether GIF search is on at an instance, and whose it is. An instance
    /// from before GIFs says no.
    pub async fn gif_settings(&self, key: &str) -> (bool, i32) {
        let Some(api) = self.api(key) else { return (false, 0) };
        match rpc!(api.gifs(), get_gif_settings(pb::GetGifSettingsRequest {})).await {
            Ok(r) => (r.enabled, r.provider),
            Err(_) => (false, 0),
        }
    }

    /// A page of results; an empty query is what's trending.
    pub async fn search_gifs(&self, key: &str, query: &str, cursor: &str) -> Result<pb::SearchGifsResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        reports::used(if query.is_empty() { "gif.trending" } else { "gif.search" });
        rpc!(api.gifs(), search_gifs(pb::SearchGifsRequest { query: query.into(), cursor: cursor.into(), limit: PAGE }))
            .await
    }

    /// Moods to browse, each with a GIF to show.
    pub async fn gif_categories(&self, key: &str) -> Result<Vec<pb::GifCategory>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.gifs(), list_gif_categories(pb::ListGifCategoriesRequest {})).await?.categories)
    }

    /// A search result stored on the instance, or one of your uploads checked, ready to send.
    pub async fn prepare_gif(&self, key: &str, from: pb::prepare_gif_request::From) -> Result<pb::MessageGif, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.gifs(), prepare_gif(pb::PrepareGifRequest { from: Some(from) })).await?;
        res.gif.ok_or_else(|| Problem::new(tonic::Code::Internal, "The instance sent no GIF back."))
    }

    /// Your saved GIFs (favorites and uploads), newest first.
    pub async fn saved_gifs(&self, key: &str) -> Result<Vec<pb::SavedGif>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.gifs(), list_saved_gifs(pb::ListSavedGifsRequest {})).await?.gifs)
    }

    /// Saves a GIF by its link: one in a message, a prepared one, or your upload.
    pub async fn save_gif(&self, key: &str, url: &str) -> Result<Option<pb::SavedGif>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        reports::used("gif.save");
        Ok(rpc!(api.gifs(), save_gif(pb::SaveGifRequest { url: url.into() })).await?.gif)
    }

    pub async fn unsave_gif(&self, key: &str, url: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.gifs(), delete_saved_gif(pb::DeleteSavedGifRequest { url: url.into() })).await?;
        Ok(())
    }

    /// Sends a GIF as a message of its own, and keeps it among the ones sent lately.
    pub async fn send_gif(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        gif: pb::MessageGif,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        reports::used("gif.send");
        self.set_prefs(|p| remember(p.recent_gifs.entry(key.to_owned()).or_default(), &gif));
        let sent = rpc!(
            api.messages(),
            send_message(pb::SendMessageRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                gif: Some(gif),
                ..Default::default()
            })
        )
        .await?;
        if let Some(message) = sent.message {
            self.shared.instance(key, |i| {
                store::add_shared_authors(&mut i.users, std::slice::from_ref(&message));
                if let Some(loaded) = i.messages.get_mut(channel_id) {
                    upsert_message(&mut loaded.items, message);
                }
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(url: &str, seal: &str) -> pb::MessageGif {
        pb::MessageGif { url: url.into(), seal: seal.into(), ..Default::default() }
    }

    #[test]
    fn sent_lately_keeps_each_once_newest_first() {
        let mut list = Vec::new();
        for n in 0..RECENT + 5 {
            remember(&mut list, &gif(&format!("u{n}"), "s"));
        }
        assert_eq!(list.len(), RECENT);
        assert_eq!(list[0].url, format!("u{}", RECENT + 4));
        remember(&mut list, &gif("u10", "s"));
        assert_eq!(list[0].url, "u10");
        assert_eq!(list.iter().filter(|g| g.url == "u10").count(), 1);
        assert_eq!(list.len(), RECENT);
        // Without a seal it couldn't be sent again.
        remember(&mut list, &gif("plain", ""));
        assert_ne!(list[0].url, "plain");
    }

    #[test]
    fn gifs_shrink_to_fit_but_never_stretch() {
        assert_eq!(fit(200, 100), (200.0, 100.0));
        assert_eq!(fit(640, 320), (320.0, 160.0));
        assert_eq!(fit(100, 520), (50.0, 260.0));
        // A sliver still gets something to point at; no size reads as square.
        assert_eq!(fit(2000, 10), (320.0, 32.0));
        assert_eq!(fit(0, 0), (48.0, 32.0));
    }

    #[test]
    fn tiles_go_in_the_shorter_column() {
        let (placed, height) = masonry(&[1.0, 2.0, 0.5, 1.0], 228.0, 6.0, 8.0);
        // Two 103-wide columns.
        assert_eq!(placed[0], (8.0, 8.0, 103.0, 103.0));
        assert_eq!(placed[1], (117.0, 8.0, 103.0, 206.0));
        // The left is shorter now, twice.
        assert_eq!(placed[2], (8.0, 117.0, 103.0, 52.0));
        assert_eq!(placed[3], (8.0, 175.0, 103.0, 103.0));
        // The taller column's foot, and the padding under it.
        assert_eq!(height, 292.0);
        // Very tall or wide ones are kept in reason, and a missing size reads as square.
        let (placed, _) = masonry(&[10.0, 0.01, f32::NAN], 228.0, 6.0, 8.0);
        assert_eq!(placed[0].3, 227.0);
        assert_eq!(placed[1].3, 46.0);
        assert_eq!(placed[2].3, 103.0);
    }
}
