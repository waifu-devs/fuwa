//! A server's custom emoji (`EmojiService`). Their pictures are uploads on
//! the instance (`MEDIA_PURPOSE_EMOJI`), counted in the server's attachments.

use tonic::{Request, Response, Status};

use super::{Api, PictureOwner, respond};
use crate::db::{is_unique_violation, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, Permission, emoji_service_server::EmojiService};
use crate::servers::{self as store, Audit, Payload, UsageChange};

/// 2 to 32 letters, digits and underscores, as Discord has it.
fn checked_name(name: &str) -> Result<String> {
    let name = name.trim().trim_matches(':');
    if !(2..=32).contains(&name.len()) || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(Error::invalid("emoji names are 2 to 32 letters, digits and underscores"));
    }
    Ok(name.to_string())
}

/// Refuses a name another of the server's emoji has, whatever its case.
async fn check_free(conn: &turso::Connection, name: &str, except: &str) -> Result<()> {
    let taken =
        query_one(conn, "SELECT 1 FROM emojis WHERE lower(name) = lower(?1) AND id <> ?2", (name, except), |r| {
            r.get::<i64>(0)
        })
        .await?;
    match taken {
        Some(_) => Err(Error::AlreadyExists(format!("there's already an emoji called :{name}:"))),
        None => Ok(()),
    }
}

fn name_taken(err: Error, name: &str) -> Error {
    if is_unique_violation(&err) {
        Error::AlreadyExists(format!("there's already an emoji called :{name}:"))
    } else {
        err
    }
}

/// Every emoji the server has now, as the event members get.
async fn changed(conn: &turso::Connection, server_id: &str, events: &mut Vec<Payload>) -> Result<()> {
    let emojis = store::load_emojis(conn, server_id).await?;
    events.push(Payload::EmojisUpdated(pb::EmojisUpdated { emojis }));
    Ok(())
}

#[tonic::async_trait]
impl EmojiService for Api {
    async fn list_emojis(
        &self,
        request: Request<pb::ListEmojisRequest>,
    ) -> Result<Response<pb::ListEmojisResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.membership(&account, &request.get_ref().server_id).await?.sdb;
                Ok(pb::ListEmojisResponse { emojis: store::load_emojis(&sdb.read()?, &sdb.id).await? })
            }
            .await,
        )
    }

    async fn create_emoji(
        &self,
        request: Request<pb::CreateEmojiRequest>,
    ) -> Result<Response<pb::CreateEmojiResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageEmoji).await?.sdb;
                let name = checked_name(&req.name)?;
                let upload = self
                    .check_upload(&account, pb::MediaPurpose::Emoji, req.url.trim(), Some(&sdb.id))
                    .await?
                    .ok_or_else(|| Error::invalid("upload the emoji's picture to this fuwa server first"))?;
                let limits = sdb.limits(&self.app.settings().limits).await?;
                if let Some(limit) = limits.attachment_bytes
                    && sdb.usage().await?.attachment_bytes + upload.size > limit
                {
                    return Err(Error::ResourceExhausted("this server is out of room for files".into()));
                }
                let emoji = sdb
                    .write(&account.id, async |conn, events| {
                        check_free(conn, &name, "").await?;
                        let count = store::usage_count(conn, "emojis").await?;
                        if let Some(limit) = limits.emojis
                            && count >= limit
                        {
                            return Err(Error::ResourceExhausted(format!("this server is full of emoji ({limit})")));
                        }
                        let now = now_ms();
                        let emoji = pb::Emoji {
                            id: new_id(),
                            server_id: sdb.id.clone(),
                            name: name.clone(),
                            url: upload.url.clone(),
                            animated: upload.content_type == "image/gif",
                            creator_id: account.id.clone(),
                            size: upload.size,
                            created_at: Some(timestamp(now)),
                        };
                        conn.execute(
                            "INSERT INTO emojis (id, name, url, animated, creator_id, size, created_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            (
                                emoji.id.as_str(),
                                emoji.name.as_str(),
                                emoji.url.as_str(),
                                emoji.animated,
                                emoji.creator_id.as_str(),
                                emoji.size,
                                now,
                            ),
                        )
                        .await
                        .map_err(|err| name_taken(err.into(), &name))?;
                        // The emoji total is one row, so emoji added at once can't pass the cap together.
                        conn.execute("UPDATE usage SET emojis = emojis + 1, updated_at = ?1 WHERE id = 1", [now])
                            .await?;
                        store::add_usage(
                            conn,
                            UsageChange { attachments: 1, attachment_bytes: emoji.size, ..Default::default() },
                        )
                        .await?;
                        store::audit(
                            conn,
                            &account.id,
                            Audit::new(pb::AuditAction::EmojiCreate, &emoji.id).change("name", "", &name),
                        )
                        .await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok(emoji)
                    })
                    .await?;
                self.keep_picture(Some(&upload.id), Some(&sdb.id)).await;
                Ok(pb::CreateEmojiResponse { emoji: Some(emoji) })
            }
            .await,
        )
    }

    async fn update_emoji(
        &self,
        request: Request<pb::UpdateEmojiRequest>,
    ) -> Result<Response<pb::UpdateEmojiResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageEmoji).await?.sdb;
                let name = checked_name(&req.name)?;
                let emoji = sdb
                    .write(&account.id, async |conn, events| {
                        let mut emoji = store::load_emojis(conn, &sdb.id)
                            .await?
                            .into_iter()
                            .find(|e| e.id == req.emoji_id)
                            .ok_or(Error::NotFound("emoji"))?;
                        if emoji.name == name {
                            return Ok(emoji);
                        }
                        check_free(conn, &name, &emoji.id).await?;
                        conn.execute("UPDATE emojis SET name = ?2 WHERE id = ?1", (emoji.id.as_str(), name.as_str()))
                            .await
                            .map_err(|err| name_taken(err.into(), &name))?;
                        let entry =
                            Audit::new(pb::AuditAction::EmojiUpdate, &emoji.id).change("name", &emoji.name, &name);
                        store::audit(conn, &account.id, entry).await?;
                        emoji.name = name.clone();
                        changed(conn, &sdb.id, events).await?;
                        Ok(emoji)
                    })
                    .await?;
                Ok(pb::UpdateEmojiResponse { emoji: Some(emoji) })
            }
            .await,
        )
    }

    async fn delete_emoji(
        &self,
        request: Request<pb::DeleteEmojiRequest>,
    ) -> Result<Response<pb::DeleteEmojiResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageEmoji).await?.sdb;
                let emoji = sdb
                    .write(&account.id, async |conn, events| {
                        let emoji = store::load_emojis(conn, &sdb.id)
                            .await?
                            .into_iter()
                            .find(|e| e.id == req.emoji_id)
                            .ok_or(Error::NotFound("emoji"))?;
                        conn.execute("DELETE FROM emojis WHERE id = ?1", [emoji.id.as_str()]).await?;
                        conn.execute("UPDATE usage SET emojis = emojis - 1, updated_at = ?1 WHERE id = 1", [now_ms()])
                            .await?;
                        store::add_usage(
                            conn,
                            UsageChange { attachments: -1, attachment_bytes: -emoji.size, ..Default::default() },
                        )
                        .await?;
                        let entry = Audit::new(pb::AuditAction::EmojiDelete, &emoji.id).change("name", &emoji.name, "");
                        store::audit(conn, &account.id, entry).await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok(emoji)
                    })
                    .await?;
                self.drop_picture(&emoji.url, "", PictureOwner::Server(&sdb.id)).await;
                Ok(pb::DeleteEmojiResponse {})
            }
            .await,
        )
    }
}
