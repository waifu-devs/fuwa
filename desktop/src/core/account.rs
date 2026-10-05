//! Your account and what you set on an instance: profile and picture,
//! password, signed-in devices, notification settings, agreeing to a
//! server's rules, and making channels. The calls from `web/src/fuwa/actions.ts`
//! that aren't about messages.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use tonic::Code;

use crate::core::api::Problem;
use crate::core::{Core, notifications, store};
use crate::pb;
use crate::rpc;

/// What changes in how a server (or a channel) notifies you.
#[derive(Debug, Clone, Default)]
pub struct NotificationPatch {
    pub level: Option<pb::NotificationLevel>,
    pub suppress_everyone: Option<bool>,
    /// `Some(None)` mutes until turned back on, `Some(Some(ms))` until then, and
    /// `unmute` turns it off.
    pub mute_until: Option<Option<i64>>,
    pub unmute: bool,
}

/// What changes in your profile.
#[derive(Debug, Clone, Default)]
pub struct ProfilePatch {
    pub display_name: Option<String>,
    pub pronouns: Option<String>,
    pub bio: Option<String>,
    pub status: Option<String>,
    pub avatar_url: Option<String>,
}

/// A picture's type, from its name, as the instance wants it said.
pub fn picture_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

impl Core {
    /// Reads your notification settings again (they change on other devices without an event).
    pub async fn refresh_notifications(&self, key: &str) {
        let Some(api) = self.api(key) else { return };
        if let Ok(res) = rpc!(api.account(), get_notification_settings(pb::GetNotificationSettingsRequest {})).await {
            self.shared.instance(key, |i| {
                i.notifications =
                    res.settings.into_iter().map(|n| (notifications::key(&n.server_id, &n.channel_id), n)).collect();
            });
        }
    }

    /// Reads your status again: another app may have changed it, and that
    /// sends no event. An older instance without presence just has none.
    pub async fn refresh_presence(&self, key: &str) {
        let Some(api) = self.api(key) else { return };
        if let Ok(res) = rpc!(api.presence(), get_presence_settings(pb::GetPresenceSettingsRequest {})).await {
            self.shared.instance(key, |i| i.presence = res.settings);
        }
    }

    /// Picks your status on an instance, for every app you're signed in with there.
    pub async fn set_status(&self, key: &str, status: pb::PresenceStatus) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        // The update replaces every setting, so the rest are read fresh, never
        // from what this app last saw: sharing turned off in another app since
        // then must stay off.
        let mut settings = rpc!(api.presence(), get_presence_settings(pb::GetPresenceSettingsRequest {}))
            .await?
            .settings
            .unwrap_or_default();
        settings.status = status as i32;
        let res = rpc!(
            api.presence(),
            update_presence_settings(pb::UpdatePresenceSettingsRequest { settings: Some(settings) })
        )
        .await?;
        self.shared.instance(key, |i| i.presence = res.settings);
        Ok(())
    }

    /// Changes how a server (or one of its channels) notifies you, on every device.
    pub async fn update_notifications(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        patch: NotificationPatch,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let mut paths = Vec::new();
        if patch.level.is_some() {
            paths.push("level".to_owned());
        }
        if patch.suppress_everyone.is_some() {
            paths.push("suppress_everyone".to_owned());
        }
        if patch.mute_until.is_some() || patch.unmute {
            paths.push("muted".to_owned());
        }
        let settings = pb::NotificationSettings {
            server_id: server_id.into(),
            channel_id: channel_id.into(),
            level: patch.level.unwrap_or_default() as i32,
            suppress_everyone: patch.suppress_everyone.unwrap_or_default(),
            muted: patch.mute_until.is_some() && !patch.unmute,
            muted_until: patch
                .mute_until
                .flatten()
                .map(|ms| prost_types::Timestamp { seconds: ms / 1000, nanos: ((ms % 1000) * 1_000_000) as i32 }),
        };
        let res = rpc!(
            api.account(),
            update_notification_settings(pb::UpdateNotificationSettingsRequest {
                settings: Some(settings),
                update_mask: Some(prost_types::FieldMask { paths }),
            })
        )
        .await?;
        self.shared.instance(key, |i| {
            let k = notifications::key(server_id, channel_id);
            match res.settings.filter(|s| s.level != 0 || s.muted || s.suppress_everyone) {
                Some(saved) => i.notifications.insert(k, saved),
                None => i.notifications.remove(&k),
            };
        });
        Ok(())
    }

    /// Makes a channel (or a category) in a server.
    pub async fn create_channel(
        &self,
        key: &str,
        server_id: &str,
        name: &str,
        kind: pb::ChannelType,
        parent_id: &str,
    ) -> Result<pb::Channel, Problem> {
        let request = pb::CreateChannelRequest {
            server_id: server_id.into(),
            name: name.into(),
            r#type: kind as i32,
            parent_id: parent_id.into(),
            ..Default::default()
        };
        self.create_channel_with(key, request).await
    }

    /// A copy of a channel: its name, kind, category, topic, slow mode and who
    /// can see it, all made in one step, so the copy is never seen with other
    /// permissions than the original's.
    pub async fn duplicate_channel(
        &self,
        key: &str,
        server_id: &str,
        of: &pb::Channel,
    ) -> Result<pb::Channel, Problem> {
        let request = pb::CreateChannelRequest {
            server_id: server_id.into(),
            name: of.name.clone(),
            r#type: of.r#type,
            parent_id: of.parent_id.clone(),
            topic: of.topic.clone(),
            slowmode_seconds: of.slowmode_seconds,
            permission_overwrites: of.permission_overwrites.clone(),
        };
        let made = self.create_channel_with(key, request).await;
        if made.is_err() {
            crate::core::reports::error("context_menu.duplicate_channel", "channel");
        }
        made
    }

    async fn create_channel_with(&self, key: &str, request: pb::CreateChannelRequest) -> Result<pb::Channel, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let server_id = request.server_id.clone();
        let channel = rpc!(api.channels(), create_channel(request)).await?.channel.unwrap_or_default();
        self.shared.instance(key, |i| {
            let list = i.channels.entry(server_id.clone()).or_default();
            list.retain(|c| c.id != channel.id);
            list.push(channel.clone());
            store::sort_channels(list);
        });
        Ok(channel)
    }

    /// A server's rules, for a member who hasn't agreed to them yet.
    pub async fn server_rules(&self, key: &str, server_id: &str) -> Result<Vec<String>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.join(),
            get_join_form(pb::GetJoinFormRequest { server_id: server_id.into(), ..Default::default() })
        )
        .await?;
        Ok(res.form.map(|f| f.rules).unwrap_or_default())
    }

    /// A server's welcome screen, with only the channels you can see.
    pub async fn welcome_screen(&self, key: &str, server_id: &str) -> Result<pb::WelcomeScreen, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.join(), get_welcome_screen(pb::GetWelcomeScreenRequest { server_id: server_id.into() })).await?;
        Ok(res.welcome_screen.unwrap_or_default())
    }

    /// Agrees to a server's rules, which lets a new member talk.
    pub async fn agree_to_rules(&self, key: &str, server_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.join(), agree_to_rules(pb::AgreeToRulesRequest { server_id: server_id.into() })).await?;
        if let Some(member) = res.member {
            self.shared.instance(key, |i| {
                let Some(user) = member.user.clone() else { return };
                let list = i.members.entry(server_id.to_owned()).or_default();
                list.retain(|m| !m.user.as_ref().is_some_and(|u| u.id == user.id));
                list.push(member);
                store::sort_members(list);
            });
        }
        Ok(())
    }

    /// Someone's whole profile: pronouns, bio, banner.
    pub async fn profile(&self, key: &str, user_id: &str) -> Result<pb::Profile, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.auth(), get_profile(pb::GetProfileRequest { user_id: user_id.into() })).await?;
        let profile = res.profile.unwrap_or_default();
        if let Some(user) = &profile.user {
            self.shared.instance(key, |i| store::update_user(i, user));
        }
        Ok(profile)
    }

    pub async fn update_profile(&self, key: &str, patch: ProfilePatch) -> Result<pb::Profile, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.auth(),
            update_profile(pb::UpdateProfileRequest {
                display_name: patch.display_name,
                avatar_url: patch.avatar_url,
                pronouns: patch.pronouns,
                bio: patch.bio,
                status: patch.status,
                ..Default::default()
            })
        )
        .await?;
        if let Some(user) = &res.user {
            self.shared.instance(key, |i| {
                store::update_user(i, user);
                i.me = Some(user.clone());
            });
        }
        Ok(res.profile.unwrap_or_default())
    }

    /// Uploads a picture to the instance and gives its link, ready to set.
    pub async fn upload_picture(
        self: &Arc<Self>,
        key: &str,
        purpose: pb::MediaPurpose,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<String, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.media(),
            create_upload(pb::CreateUploadRequest {
                purpose: purpose as i32,
                content_type: content_type.into(),
                size: bytes.len() as i64,
                server_id: String::new(),
            })
        )
        .await?;
        // The bytes go to the instance's own address, whatever name it gave the link.
        let token = res.upload_url.rsplit('/').next().unwrap_or_default();
        let target = format!("{}/media/upload/{token}", api.url.trim_end_matches('/'));
        put(&target, content_type, bytes).await?;
        crate::core::reports::used("upload");
        Ok(res.media.map(|m| m.url).unwrap_or_default())
    }

    pub async fn change_password(&self, key: &str, current: &str, new: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.auth(),
            change_password(pb::ChangePasswordRequest { current_password: current.into(), new_password: new.into() })
        )
        .await?;
        Ok(())
    }

    pub async fn sessions(&self, key: &str) -> Result<Vec<pb::Session>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.account(), list_sessions(pb::ListSessionsRequest {})).await?.sessions)
    }

    pub async fn revoke_session(&self, key: &str, session_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.account(), revoke_session(pb::RevokeSessionRequest { session_id: session_id.into() })).await?;
        Ok(())
    }

    pub async fn revoke_other_sessions(&self, key: &str) -> Result<i32, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.account(), revoke_other_sessions(pb::RevokeOtherSessionsRequest {})).await?.revoked)
    }
}

/// Sends an upload's bytes: a plain `PUT` to the link the instance handed out.
async fn put(url: &str, content_type: &str, bytes: Vec<u8>) -> Result<(), Problem> {
    send(http::Method::PUT, url, content_type, bytes).await
}

fn client() -> hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    Full<Bytes>,
> {
    let roots = match hyper_rustls::HttpsConnectorBuilder::new().with_native_roots() {
        Ok(roots) => roots,
        Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
    };
    let connector = roots.https_or_http().enable_http1().build();
    hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new()).build(connector)
}

/// Fetches a file from an instance's own address, refusing one bigger than `most` bytes.
pub(crate) async fn fetch(url: &str, most: usize) -> Result<Vec<u8>, Problem> {
    let unreachable = || Problem::new(Code::Unavailable, "Couldn't reach this instance right now.");
    let request = http::Request::get(url).body(Full::new(Bytes::new())).map_err(|_| unreachable())?;
    let fetched = async {
        let response = client().request(request).await.map_err(|_| unreachable())?;
        match response.status().as_u16() {
            200 => {}
            404 | 410 => return Err(Problem::new(Code::NotFound, "That file isn't on the instance anymore.")),
            _ => return Err(unreachable()),
        }
        let body = http_body_util::Limited::new(response.into_body(), most)
            .collect()
            .await
            .map_err(|_| Problem::new(Code::ResourceExhausted, "That file is bigger than it should be."))?;
        Ok(body.to_bytes().to_vec())
    };
    tokio::time::timeout(std::time::Duration::from_secs(120), fetched)
        .await
        .map_err(|_| Problem::new(Code::DeadlineExceeded, "That took too long."))?
}

/// Sends `bytes` to an instance's own address (an upload, a webhook) and
/// turns a refusal into a [`Problem`] with what the instance said.
pub(crate) async fn send(method: http::Method, url: &str, content_type: &str, bytes: Vec<u8>) -> Result<(), Problem> {
    let unreachable = || Problem::new(Code::Unavailable, "Couldn't reach this instance right now.");
    let client = client();
    let request = http::Request::builder()
        .method(method)
        .uri(url)
        .header(http::header::CONTENT_TYPE, content_type)
        .body(Full::new(Bytes::from(bytes)))
        .map_err(|_| unreachable())?;
    let response = tokio::time::timeout(std::time::Duration::from_secs(120), client.request(request))
        .await
        .map_err(|_| Problem::new(Code::DeadlineExceeded, "That took too long."))?
        .map_err(|_| unreachable())?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.into_body().collect().await.map(|b| b.to_bytes()).unwrap_or_default();
    // Webhooks answer `{"message": …}`; uploads answer plain text.
    let text = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned))
        .unwrap_or_else(|| String::from_utf8_lossy(&body).trim().to_owned());
    let code = match status.as_u16() {
        400 | 415 => Code::InvalidArgument,
        404 | 410 => Code::NotFound,
        413 | 429 => Code::ResourceExhausted,
        _ => Code::Unavailable,
    };
    let message = if text.is_empty() { "That didn't go through.".to_owned() } else { text };
    Err(Problem::new(code, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_name_their_type() {
        assert_eq!(picture_type("me.PNG"), Some("image/png"));
        assert_eq!(picture_type("a.b.jpeg"), Some("image/jpeg"));
        assert_eq!(picture_type("notes.txt"), None);
    }
}
