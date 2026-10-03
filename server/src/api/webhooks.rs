//! A server's webhooks (`WebhookService`), and posting through one
//! (`execute_webhook`, which `crate::webhooks` serves over plain HTTP). They
//! live in the server's file (`webhooks`); changes are audit-only.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use super::messages::{WebhookMessage, check_webhook_message, insert_webhook_message};
use super::{Api, PictureOwner, respond, text, users};
use crate::app::App;
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, Permission, webhook_service_server::WebhookService};
use crate::servers::{self as store, Audit, load_channel};

/// Most webhooks one server can have, as Discord allows per channel.
const MAX_WEBHOOKS: i64 = 50;
const TOKEN_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const TOKEN_LENGTH: usize = 64;

/// A fresh secret for a webhook's address. 64 characters from 64 is 384 bits.
fn new_token() -> String {
    let mut bytes = [0u8; TOKEN_LENGTH];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    bytes.iter().map(|b| TOKEN_ALPHABET[usize::from(*b) % TOKEN_ALPHABET.len()] as char).collect()
}

/// Compares tokens without stopping at the first difference.
fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

const WEBHOOK_COLUMNS: &str = "id, channel_id, name, avatar_url, token, creator_id, created_at, last_used_at, messages";

fn webhook_row(server_id: &str) -> impl Fn(&turso::Row) -> turso::Result<pb::Webhook> + '_ {
    move |r| {
        Ok(pb::Webhook {
            id: r.get(0)?,
            server_id: server_id.to_string(),
            channel_id: r.get(1)?,
            name: r.get(2)?,
            avatar_url: r.get(3)?,
            token: r.get(4)?,
            creator_id: r.get(5)?,
            created_at: Some(timestamp(r.get(6)?)),
            last_used_at: r.get::<Option<i64>>(7)?.map(timestamp),
            messages: r.get(8)?,
        })
    }
}

async fn load_webhook(conn: &turso::Connection, server_id: &str, id: &str) -> Result<pb::Webhook> {
    query_one(conn, &format!("SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE id = ?1"), [id], webhook_row(server_id))
        .await?
        .ok_or(Error::NotFound("webhook"))
}

/// A channel webhooks can post in: a text or announcement channel of the server.
async fn postable(conn: &turso::Connection, server_id: &str, channel_id: &str) -> Result<pb::Channel> {
    let channel = load_channel(conn, server_id, channel_id).await?.ok_or(Error::NotFound("channel"))?;
    match pb::ChannelType::try_from(channel.r#type) {
        Ok(pb::ChannelType::Text | pb::ChannelType::Announcement) => Ok(channel),
        _ => Err(Error::invalid("webhooks can only post in text and announcement channels")),
    }
}

impl Api {
    /// The picture a webhook will use: none, the one it has, or a new upload of the caller's.
    async fn webhook_picture(
        &self,
        account: &crate::node::Account,
        url: &str,
        current: &str,
    ) -> Result<(String, Option<String>)> {
        let url = url.trim();
        if url.is_empty() || url == current {
            return Ok((url.to_string(), None));
        }
        let upload = self
            .check_upload(account, pb::MediaPurpose::Avatar, url)
            .await?
            .ok_or_else(|| Error::invalid("upload the webhook's picture to this fuwa server first"))?;
        Ok((upload.url, Some(upload.id)))
    }
}

#[tonic::async_trait]
impl WebhookService for Api {
    async fn list_webhooks(
        &self,
        request: Request<pb::ListWebhooksRequest>,
    ) -> Result<Response<pb::ListWebhooksResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.with(&account, &request.get_ref().server_id, Permission::ManageWebhooks).await?.sdb;
                let conn = sdb.read()?;
                let webhooks = query_all(
                    &conn,
                    &format!("SELECT {WEBHOOK_COLUMNS} FROM webhooks ORDER BY id"),
                    (),
                    webhook_row(&sdb.id),
                )
                .await?;
                let creators =
                    users(&conn, &webhooks.iter().map(|w| w.creator_id.as_str()).collect::<Vec<_>>()).await?;
                Ok(pb::ListWebhooksResponse { webhooks, creators })
            }
            .await,
        )
    }

    async fn create_webhook(
        &self,
        request: Request<pb::CreateWebhookRequest>,
    ) -> Result<Response<pb::CreateWebhookResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageWebhooks).await?.sdb;
                let name = text("name", &req.name, 1, 80)?;
                let (avatar_url, upload) = self.webhook_picture(&account, &req.avatar_url, "").await?;
                let webhook = sdb
                    .write(&account.id, async |conn, _events| {
                        let channel = postable(conn, &sdb.id, &req.channel_id).await?;
                        let count = query_one(conn, "SELECT count(*) FROM webhooks", (), |r| r.get::<i64>(0)).await?;
                        if count.unwrap_or_default() >= MAX_WEBHOOKS {
                            return Err(Error::ResourceExhausted(format!(
                                "a server can have at most {MAX_WEBHOOKS} webhooks"
                            )));
                        }
                        let now = now_ms();
                        let webhook = pb::Webhook {
                            id: new_id(),
                            server_id: sdb.id.clone(),
                            channel_id: channel.id.clone(),
                            name: name.clone(),
                            avatar_url: avatar_url.clone(),
                            creator_id: account.id.clone(),
                            created_at: Some(timestamp(now)),
                            token: new_token(),
                            last_used_at: None,
                            messages: 0,
                        };
                        conn.execute(
                            "INSERT INTO webhooks (id, channel_id, name, avatar_url, token, creator_id, created_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            (
                                webhook.id.as_str(),
                                webhook.channel_id.as_str(),
                                webhook.name.as_str(),
                                webhook.avatar_url.as_str(),
                                webhook.token.as_str(),
                                webhook.creator_id.as_str(),
                                now,
                            ),
                        )
                        .await?;
                        let entry = Audit::new(pb::AuditAction::WebhookCreate, &webhook.id)
                            .channel(&channel.name)
                            .change("name", "", &name);
                        store::audit(conn, &account.id, entry).await?;
                        Ok(webhook)
                    })
                    .await?;
                self.keep_picture(upload.as_deref(), Some(&sdb.id)).await;
                Ok(pb::CreateWebhookResponse { webhook: Some(webhook) })
            }
            .await,
        )
    }

    async fn update_webhook(
        &self,
        request: Request<pb::UpdateWebhookRequest>,
    ) -> Result<Response<pb::UpdateWebhookResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageWebhooks).await?.sdb;
                let name = text("name", &req.name, 1, 80)?;
                let current = load_webhook(&sdb.read()?, &sdb.id, &req.webhook_id).await?;
                let (avatar_url, upload) = self.webhook_picture(&account, &req.avatar_url, &current.avatar_url).await?;
                let (before, after) = sdb
                    .write(&account.id, async |conn, _events| {
                        let before = load_webhook(conn, &sdb.id, &req.webhook_id).await?;
                        let channel_id = if req.channel_id.is_empty() { &before.channel_id } else { &req.channel_id };
                        let channel = postable(conn, &sdb.id, channel_id).await?;
                        conn.execute(
                            "UPDATE webhooks SET name = ?2, avatar_url = ?3, channel_id = ?4 WHERE id = ?1",
                            (before.id.as_str(), name.as_str(), avatar_url.as_str(), channel.id.as_str()),
                        )
                        .await?;
                        let after = pb::Webhook {
                            name: name.clone(),
                            avatar_url: avatar_url.clone(),
                            channel_id: channel.id.clone(),
                            ..before.clone()
                        };
                        let entry = Audit::new(pb::AuditAction::WebhookUpdate, &before.id)
                            .channel(&channel.name)
                            .change("name", &before.name, &after.name)
                            .change("avatar_url", &before.avatar_url, &after.avatar_url)
                            .change("channel_id", &before.channel_id, &after.channel_id);
                        if !entry.changes.is_empty() {
                            store::audit(conn, &account.id, entry).await?;
                        }
                        Ok((before, after))
                    })
                    .await?;
                self.keep_picture(upload.as_deref(), Some(&sdb.id)).await;
                self.drop_picture(&before.avatar_url, &after.avatar_url, PictureOwner::Server(&sdb.id)).await;
                Ok(pb::UpdateWebhookResponse { webhook: Some(after) })
            }
            .await,
        )
    }

    async fn reset_webhook_token(
        &self,
        request: Request<pb::ResetWebhookTokenRequest>,
    ) -> Result<Response<pb::ResetWebhookTokenResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageWebhooks).await?.sdb;
                let webhook = sdb
                    .write(&account.id, async |conn, _events| {
                        let mut webhook = load_webhook(conn, &sdb.id, &req.webhook_id).await?;
                        webhook.token = new_token();
                        conn.execute(
                            "UPDATE webhooks SET token = ?2 WHERE id = ?1",
                            (webhook.id.as_str(), webhook.token.as_str()),
                        )
                        .await?;
                        let channel = load_channel(conn, &sdb.id, &webhook.channel_id).await?.unwrap_or_default();
                        let mut entry = Audit::new(pb::AuditAction::WebhookUpdate, &webhook.id).channel(&channel.name);
                        // Never the token itself: the audit log is for anyone who can view it.
                        entry.changes.push(pb::AuditChange { field: "token".into(), ..Default::default() });
                        store::audit(conn, &account.id, entry).await?;
                        Ok(webhook)
                    })
                    .await?;
                Ok(pb::ResetWebhookTokenResponse { webhook: Some(webhook) })
            }
            .await,
        )
    }

    async fn delete_webhook(
        &self,
        request: Request<pb::DeleteWebhookRequest>,
    ) -> Result<Response<pb::DeleteWebhookResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageWebhooks).await?.sdb;
                let webhook = sdb
                    .write(&account.id, async |conn, _events| {
                        let webhook = load_webhook(conn, &sdb.id, &req.webhook_id).await?;
                        conn.execute("DELETE FROM webhooks WHERE id = ?1", [webhook.id.as_str()]).await?;
                        let channel = load_channel(conn, &sdb.id, &webhook.channel_id).await?.unwrap_or_default();
                        let entry = Audit::new(pb::AuditAction::WebhookDelete, &webhook.id)
                            .channel(&channel.name)
                            .change("name", &webhook.name, "");
                        store::audit(conn, &account.id, entry).await?;
                        Ok(webhook)
                    })
                    .await?;
                self.drop_picture(&webhook.avatar_url, "", PictureOwner::Server(&sdb.id)).await;
                Ok(pb::DeleteWebhookResponse {})
            }
            .await,
        )
    }
}

/// What a post to a webhook's address asks for.
pub struct WebhookPost {
    pub content: String,
    /// A name for this message only; the webhook's own when empty.
    pub username: String,
    /// A picture for this message only; the webhook's own when empty.
    pub avatar_url: String,
    pub embeds: Vec<pb::Embed>,
}

/// Whether `token` is the webhook's, as an error when it isn't. Unknown
/// webhooks and wrong tokens are both "not found".
pub async fn verify_webhook(app: &Arc<App>, server_id: &str, webhook_id: &str, token: &str) -> Result<()> {
    let sdb = app.servers.get(server_id).await?;
    match load_webhook(&sdb.read()?, &sdb.id, webhook_id).await {
        Ok(webhook) if same_token(&webhook.token, token) => Ok(()),
        Ok(_) | Err(Error::NotFound(_)) => Err(Error::NotFound("webhook")),
        Err(err) => Err(err),
    }
}

/// Posts a message through a webhook, if `token` is its token. Unknown
/// webhooks and wrong tokens are both "not found", so neither can be probed.
pub async fn execute_webhook(
    app: &Arc<App>,
    server_id: &str,
    webhook_id: &str,
    token: &str,
    post: WebhookPost,
) -> Result<pb::Message> {
    let sdb = app.servers.get(server_id).await?;
    let webhook = load_webhook(&sdb.read()?, &sdb.id, webhook_id).await.ok().filter(|w| same_token(&w.token, token));
    let webhook = webhook.ok_or(Error::NotFound("webhook"))?;
    let username = match post.username.trim() {
        "" => webhook.name.clone(),
        name => text("username", name, 1, 80)?,
    };
    let avatar_url = match post.avatar_url.trim() {
        "" => webhook.avatar_url.clone(),
        url => super::url("avatar_url", url)?,
    };
    let mut message = WebhookMessage {
        content: post.content,
        embeds: post.embeds,
        author: pb::MessageWebhook { webhook_id: webhook.id.clone(), name: username, avatar_url },
    };
    check_webhook_message(&mut message)?;
    let limits = sdb.limits(&app.settings().limits).await?;
    if let Some(limit) = limits.storage_bytes
        && sdb.storage_bytes() >= limit
    {
        return Err(Error::ResourceExhausted("this server is out of storage".into()));
    }
    sdb.write(&webhook.id, async |conn, events| {
        // Checked again inside the write: it may have been reset, moved or deleted since.
        let current = load_webhook(conn, &sdb.id, &webhook.id).await?;
        if !same_token(&current.token, token) {
            return Err(Error::NotFound("webhook"));
        }
        postable(conn, &sdb.id, &current.channel_id).await?;
        let now = now_ms();
        conn.execute(
            "UPDATE webhooks SET last_used_at = ?2, messages = messages + 1 WHERE id = ?1",
            (current.id.as_str(), now),
        )
        .await?;
        insert_webhook_message(conn, &sdb.id, &current.channel_id, message.clone(), now, events).await
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_compared_whole() {
        let token = new_token();
        assert_eq!(token.len(), TOKEN_LENGTH);
        assert_ne!(token, new_token());
        assert!(same_token(&token, &token.clone()));
        assert!(!same_token(&token, &token[1..]));
        assert!(!same_token("abc", "abd"));
    }
}
