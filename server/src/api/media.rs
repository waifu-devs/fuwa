use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::app::Link;
use crate::auth;
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::media::{self, MediaRow};
use crate::node::Account;
use crate::pb::{self, media_service_server::MediaService};

#[tonic::async_trait]
impl MediaService for Api {
    async fn create_upload(
        &self,
        request: Request<pb::CreateUploadRequest>,
    ) -> Result<Response<pb::CreateUploadResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let purpose = match pb::MediaPurpose::try_from(req.purpose) {
                    Ok(
                        purpose @ (pb::MediaPurpose::Avatar
                        | pb::MediaPurpose::Banner
                        | pb::MediaPurpose::ServerIcon
                        | pb::MediaPurpose::Emoji
                        | pb::MediaPurpose::Background
                        | pb::MediaPurpose::Gif),
                    ) => purpose,
                    _ => {
                        return Err(Error::invalid(
                            "an upload is an avatar, a banner, a server icon, an emoji, a background or a GIF",
                        ));
                    }
                };
                if !media::PICTURE_TYPES.contains(&req.content_type.as_str()) {
                    return Err(Error::invalid("pictures can be PNG, JPEG, GIF, WebP or AVIF"));
                }
                if purpose == pb::MediaPurpose::Gif && req.content_type != "image/gif" {
                    return Err(Error::invalid("a GIF upload is a GIF"));
                }
                if req.size <= 0 {
                    return Err(Error::invalid("that file is empty"));
                }
                let server_id = match req.server_id.trim() {
                    "" => None,
                    id => {
                        if !matches!(
                            purpose,
                            pb::MediaPurpose::ServerIcon | pb::MediaPurpose::Emoji | pb::MediaPurpose::Avatar
                        ) {
                            return Err(Error::invalid(
                                "only icons, emoji and webhook pictures are uploaded for a server",
                            ));
                        }
                        let id = crate::id::parse_id("server_id", id)?;
                        if !self.app.index.is_member(&account.id, &id) {
                            return Err(Error::NotFound("server"));
                        }
                        Some(id)
                    }
                };
                let settings = self.app.settings();
                if let Some(cap) = settings.limits.picture_upload_bytes
                    && req.size > cap
                {
                    return Err(Error::ResourceExhausted(format!(
                        "pictures can be at most {} here",
                        media::size_label(cap)
                    )));
                }
                let row = MediaRow {
                    id: media::new_id(),
                    account_id: account.id.clone(),
                    purpose,
                    content_type: req.content_type,
                    size: req.size,
                    stored: false,
                    used: false,
                    server_id: server_id.clone(),
                };
                let token = auth::new_token();
                let expires_at = now_ms() + media::UPLOAD_TTL_MS;
                self.app
                    .node()?
                    .reserve_media(
                        &row,
                        &auth::hash_token(&token),
                        expires_at,
                        settings.limits.picture_upload_bytes_per_day,
                    )
                    .await?;
                let base = &settings.public_url;
                // A server's picture on a split instance goes straight to the
                // shard holding the server (docs/regions.md).
                let upload_url = match (&server_id, &self.app.link) {
                    (Some(server_id), Link::Directory(_)) => format!("{base}/media/servers/{server_id}/upload/{token}"),
                    _ => format!("{base}/media/upload/{token}"),
                };
                Ok(pb::CreateUploadResponse {
                    upload_url,
                    expires_at: Some(timestamp(expires_at)),
                    media: Some(pb::Media {
                        url: format!("{base}/media/{}", row.id),
                        id: row.id,
                        content_type: row.content_type,
                        size: row.size,
                    }),
                })
            }
            .await,
        )
    }

    async fn keep_background(
        &self,
        request: Request<pb::KeepBackgroundRequest>,
    ) -> Result<Response<pb::KeepBackgroundResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let url = request.into_inner().url;
                let Some(upload) = self.check_upload(&account, pb::MediaPurpose::Background, &url, None).await? else {
                    return Err(Error::invalid("upload the background to this instance first"));
                };
                let node = self.app.node()?;
                let kept = node.backgrounds(&account.id).await?;
                if !kept.iter().any(|row| row.id == upload.id) && kept.len() >= media::MAX_BACKGROUNDS {
                    return Err(Error::ResourceExhausted(format!(
                        "you can keep {} backgrounds here; delete one first",
                        media::MAX_BACKGROUNDS
                    )));
                }
                node.use_media(&upload.id, None).await?;
                Ok(pb::KeepBackgroundResponse {
                    media: Some(self.served(&upload.id, upload.content_type, upload.size)),
                })
            }
            .await,
        )
    }

    async fn list_backgrounds(
        &self,
        request: Request<pb::ListBackgroundsRequest>,
    ) -> Result<Response<pb::ListBackgroundsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let rows = self.app.node()?.backgrounds(&account.id).await?;
                Ok(pb::ListBackgroundsResponse {
                    backgrounds: rows.into_iter().map(|row| self.served(&row.id, row.content_type, row.size)).collect(),
                })
            }
            .await,
        )
    }

    async fn delete_background(
        &self,
        request: Request<pb::DeleteBackgroundRequest>,
    ) -> Result<Response<pb::DeleteBackgroundResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let url = request.into_inner().url;
                let Some(id) = media::id_in_url(&url) else {
                    return Err(Error::invalid("that isn't a background here"));
                };
                match self.app.node()?.media(&id).await? {
                    Some(row) if row.account_id == account.id && row.purpose == pb::MediaPurpose::Background => {
                        self.app.delete_media(&[id]).await?;
                    }
                    // Already gone: deleting twice is fine.
                    None => {}
                    Some(_) => return Err(Error::denied("that background isn't yours")),
                }
                Ok(pb::DeleteBackgroundResponse {})
            }
            .await,
        )
    }
}

impl Api {
    /// An upload as apps see it, at this instance's public address.
    fn served(&self, id: &str, content_type: String, size: i64) -> pb::Media {
        let base = self.app.settings().public_url.clone();
        pb::Media { url: format!("{base}/media/{id}"), id: id.to_string(), content_type, size }
    }

    /// Checks a picture link about to be set; see [`App::check_picture`](crate::app::App::check_picture).
    pub(super) async fn check_picture(
        &self,
        account: &Account,
        purpose: pb::MediaPurpose,
        url: &str,
        server_id: Option<&str>,
    ) -> Result<Option<String>> {
        self.app.check_picture(&account.id, purpose, url, server_id).await
    }

    /// Like `check_picture`, with the upload's size and type.
    pub(super) async fn check_upload(
        &self,
        account: &Account,
        purpose: pb::MediaPurpose,
        url: &str,
        server_id: Option<&str>,
    ) -> Result<Option<pb::Media>> {
        self.app.check_upload(&account.id, purpose, url, server_id).await
    }

    /// Marks a checked upload as used. On a split instance a server's
    /// shard then takes the server's pictures (docs/regions.md).
    pub(super) async fn keep_picture(&self, id: Option<&str>, server_id: Option<&str>) {
        self.app.keep_picture(id, server_id).await;
        if let (Some(id), Some(server_id), Link::Shard(_)) = (id, server_id, &self.app.link) {
            crate::cluster::pictures::take_soon(self.app.clone(), server_id.to_string(), id.to_string());
        }
    }

    /// Deletes the picture a change replaced, if it belonged to what changed.
    pub(super) async fn drop_picture(&self, old_url: &str, new_url: &str, owner: PictureOwner<'_>) {
        self.app.drop_picture(old_url, new_url, owner).await
    }
}

/// Whose picture a replaced link was.
#[derive(Clone, Copy)]
pub enum PictureOwner<'a> {
    Account(&'a str, pb::MediaPurpose),
    Server(&'a str),
}
