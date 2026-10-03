//! Background pictures (fuwa themes' backdrops) kept on an instance: up to
//! 24 per account, uploaded like any picture and then kept, listed and
//! removed (`MediaService`'s backgrounds). The settings that use them stay
//! on this computer.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty, Limited};
use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::themes::{MAX_FILE_PICTURE_BYTES, Picture};
use crate::pb;
use crate::rpc;

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

impl Core {
    /// Your backgrounds on an instance, newest first, as their links.
    pub async fn backgrounds(self: &Arc<Self>, key: &str) -> Result<Vec<String>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.media(), list_backgrounds(pb::ListBackgroundsRequest {})).await?;
        Ok(res.backgrounds.into_iter().map(|m| m.url).collect())
    }

    /// Uploads a picture as a background and keeps it. Returns its link.
    pub async fn upload_background(self: &Arc<Self>, key: &str, picture: Picture) -> Result<String, Problem> {
        let url = self.upload_picture(key, pb::MediaPurpose::Background, &picture.content_type, picture.bytes).await?;
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.media(), keep_background(pb::KeepBackgroundRequest { url: url.clone() })).await?;
        Ok(res.media.map(|m| m.url).unwrap_or(url))
    }

    pub async fn delete_background(self: &Arc<Self>, key: &str, url: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.media(), delete_background(pb::DeleteBackgroundRequest { url: url.into() })).await?;
        Ok(())
    }

    /// A background's bytes, for putting in a theme file. Only from an
    /// instance you added, asked at the address fuwa talks to it on.
    pub async fn background_bytes(self: &Arc<Self>, link: &str) -> Option<Picture> {
        let wanted = url::Url::parse(link).ok()?;
        let keys = self.shared.read(|s| s.order.clone());
        let api = keys.iter().filter_map(|k| self.api(k)).find(|api| {
            let ours = url::Url::parse(&api.url).ok();
            let theirs = self.shared.read(|s| {
                s.instance(&crate::core::api::instance_key(&api.url)).and_then(|i| url::Url::parse(&i.url).ok())
            });
            [ours, theirs].into_iter().flatten().any(|u| u.host_str() == wanted.host_str())
        })?;
        let target = format!("{}{}", api.url.trim_end_matches('/'), wanted.path());
        get(&target).await
    }
}

async fn get(url: &str) -> Option<Picture> {
    let roots = match hyper_rustls::HttpsConnectorBuilder::new().with_native_roots() {
        Ok(roots) => roots,
        Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
    };
    let connector = roots.https_or_http().enable_http1().build();
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build::<_, Empty<Bytes>>(connector);
    let request = http::Request::get(url).body(Empty::new()).ok()?;
    let response =
        tokio::time::timeout(std::time::Duration::from_secs(60), client.request(request)).await.ok()?.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or_default().trim().to_owned())?;
    let bytes = Limited::new(response.into_body(), MAX_FILE_PICTURE_BYTES).collect().await.ok()?.to_bytes();
    Some(Picture { content_type, bytes: bytes.to_vec() })
}
