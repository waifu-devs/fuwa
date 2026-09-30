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
                        purpose @ (pb::MediaPurpose::Avatar | pb::MediaPurpose::Banner | pb::MediaPurpose::ServerIcon),
                    ) => purpose,
                    _ => return Err(Error::invalid("an upload is an avatar, a banner or a server icon")),
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
                self.app.node.reserve_media(&row, &auth::hash_token(&token), expires_at).await?;
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
    /// Checks a picture link about to be set. A link to one of this
    /// instance's uploads must be to one the caller uploaded, for this
    /// purpose, and stored; its id comes back so `keep_picture` can mark it
    /// used once the change is saved. Any other link passes as it is.
    pub(super) async fn check_picture(
        &self,
        account: &Account,
        purpose: pb::MediaPurpose,
        url: &str,
    ) -> Result<Option<String>> {
        let Some(id) = media::id_in_url(url) else { return Ok(None) };
        let Some(row) = self.app.node.media(&id).await? else { return Ok(None) };
        if row.account_id != account.id || row.purpose != purpose {
            return Err(Error::denied("upload that picture yourself to use it here"));
        }
        if !row.stored {
            return Err(Error::FailedPrecondition("that picture hasn't finished uploading".into()));
        }
        Ok(Some(id))
    }

    /// Marks a checked upload as used. A failure only means it may be swept
    /// later, so it's logged.
    pub(super) async fn keep_picture(&self, id: Option<&str>, server_id: Option<&str>) {
        if let Some(id) = id
            && let Err(err) = self.app.node.use_media(id, server_id).await
        {
            tracing::warn!(media = %id, error = %err, "couldn't mark a picture as used");
        }
    }

    /// Deletes the picture a change replaced, if it was one of this
    /// instance's uploads and belonged to what changed: the account's own
    /// avatar or banner, or the server's icon.
    pub(super) async fn drop_picture(&self, old_url: &str, new_url: &str, owner: PictureOwner<'_>) {
        if old_url == new_url {
            return;
        }
        let Some(id) = media::id_in_url(old_url) else { return };
        let row = match self.app.node.media(&id).await {
            Ok(Some(row)) => row,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(media = %id, error = %err, "couldn't look up a replaced picture");
                return;
            }
        };
        let belongs = match owner {
            PictureOwner::Account(account_id, purpose) => row.account_id == account_id && row.purpose == purpose,
            PictureOwner::Server(server_id) => {
                row.purpose == pb::MediaPurpose::ServerIcon && row.server_id.as_deref() == Some(server_id)
            }
        };
        if belongs && let Err(err) = self.app.delete_media(&[id]).await {
            tracing::warn!(media = %row.id, error = %err, "couldn't delete a replaced picture");
        }
    }
}

/// Whose picture a replaced link was.
#[derive(Clone, Copy)]
pub(super) enum PictureOwner<'a> {
    Account(&'a str, pb::MediaPurpose),
    Server(&'a str),
}
