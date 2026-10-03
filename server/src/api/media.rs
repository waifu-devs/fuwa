use tonic::{Request, Response, Status};

use super::{Api, respond};
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
                        | pb::MediaPurpose::Emoji),
                    ) => purpose,
                    _ => return Err(Error::invalid("an upload is an avatar, a banner, a server icon or an emoji")),
                };
                if !media::PICTURE_TYPES.contains(&req.content_type.as_str()) {
                    return Err(Error::invalid("pictures can be PNG, JPEG, GIF, WebP or AVIF"));
                }
                if req.size <= 0 {
                    return Err(Error::invalid("that file is empty"));
                }
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
                    server_id: None,
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
                Ok(pb::CreateUploadResponse {
                    upload_url: format!("{base}/media/upload/{token}"),
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
}

impl Api {
    /// Checks a picture link about to be set; see [`App::check_picture`](crate::app::App::check_picture).
    pub(super) async fn check_picture(
        &self,
        account: &Account,
        purpose: pb::MediaPurpose,
        url: &str,
    ) -> Result<Option<String>> {
        self.app.check_picture(&account.id, purpose, url).await
    }

    /// Like `check_picture`, with the upload's size and type.
    pub(super) async fn check_upload(
        &self,
        account: &Account,
        purpose: pb::MediaPurpose,
        url: &str,
    ) -> Result<Option<pb::Media>> {
        self.app.check_upload(&account.id, purpose, url).await
    }

    /// Marks a checked upload as used.
    pub(super) async fn keep_picture(&self, id: Option<&str>, server_id: Option<&str>) {
        self.app.keep_picture(id, server_id).await
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
