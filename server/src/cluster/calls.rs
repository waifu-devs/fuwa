//! The calls that cross between parts: made right here when one process runs
//! everything, and over the cluster protocol when the instance is split.
//! Handlers use these, so each runs the same whichever part it's in.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tonic::metadata::MetadataMap;

use super::shard;
use crate::api::PictureOwner;
use crate::app::{App, Link};
use crate::auth::Viewer;
use crate::cpb;
use crate::error::{Error, Result};
use crate::media;
use crate::pb;
use crate::servers::NewServer;

/// For a secure channel: the device the caller's session registered, if it
/// did, and the signed-in devices of the people asked about, as `(device id,
/// account id)`.
pub struct SecureDevices {
    pub caller: Option<String>,
    pub devices: Vec<(String, String)>,
}

/// An agent, as adding it to a server needs it.
pub struct FoundAgent {
    pub account: crate::node::Account,
    pub owner_id: String,
    pub public: bool,
}

impl App {
    // ─────────────── Asked of the directory ───────────────

    /// Who a request comes from.
    pub async fn authenticate(&self, metadata: &MetadataMap) -> Result<Viewer> {
        match &self.link {
            Link::Shard(link) => link.authenticate(metadata).await,
            _ => crate::auth::authenticate(self.node()?, self.config.admin_token.as_deref(), metadata).await,
        }
    }

    /// Whether a session is still signed in.
    pub async fn session_live(&self, token_hash: &str) -> Result<bool> {
        match &self.link {
            Link::Shard(link) => {
                let request = cpb::SessionLiveRequest { token_hash: token_hash.to_string() };
                Ok(link.directory().session_live(request).await?.into_inner().live)
            }
            _ => self.node()?.session_live(token_hash).await,
        }
    }

    /// Records a server's new profile after a committed change.
    pub async fn server_changed(&self, server: &pb::Server) {
        match &self.link {
            Link::Shard(link) => {
                let request = cpb::IndexServerRequest { server: Some(server.clone()) };
                link.tell("a server changed", async |mut d| d.index_server(request).await).await;
            }
            _ => self.index.update(server.clone()),
        }
    }

    /// Records someone joining (or leaving) a server.
    pub async fn membership_changed(&self, account_id: &str, server_id: &str, joined: bool) {
        match &self.link {
            Link::Shard(link) => {
                let request = cpb::IndexMembershipRequest {
                    server_id: server_id.to_string(),
                    account_id: account_id.to_string(),
                    joined,
                };
                link.tell("about a membership", async |mut d| d.index_membership(request).await).await;
            }
            _ if joined => {
                self.index.join(account_id, server_id);
                self.presence.joined(&self.index, account_id, server_id);
            }
            _ => {
                self.index.leave(account_id, server_id);
                self.presence.left(&self.index, account_id, server_id);
            }
        }
    }

    /// Records an invite made (`exists`), or deleted, used up or expired.
    pub async fn index_invite(&self, server_id: &str, code: &str, exists: bool) {
        match &self.link {
            Link::Shard(link) => {
                let request =
                    cpb::IndexInviteRequest { server_id: server_id.to_string(), code: code.to_string(), exists };
                link.tell("about an invite", async |mut d| d.index_invite(request).await).await;
            }
            _ => self.index.index_invite(server_id, code, exists),
        }
    }

    /// Takes a deleted server out of the index, and tells its members who
    /// they no longer share a server with.
    fn forget_server_presence(&self, server_id: &str) {
        let members = self.index.members_where(server_id, |_| true);
        self.index.remove(server_id);
        self.presence.server_gone(&self.index, &members);
    }

    /// Forgets a deleted server: its place in the index, and notification
    /// settings for it.
    pub async fn server_gone(&self, server_id: &str) {
        match &self.link {
            Link::Shard(link) => {
                let request = cpb::DropServerRequest { server_id: server_id.to_string() };
                link.tell("a server was deleted", async |mut d| d.drop_server(request).await).await;
            }
            Link::Directory(_) => {
                self.forget_server_presence(server_id);
                if let Err(err) = async { self.node()?.place(server_id, None).await }.await {
                    tracing::warn!(server = %server_id, error = %err, "couldn't forget where a deleted server was");
                }
                self.forget_notifications(server_id, None, None).await;
                self.drop_server_media(server_id).await;
            }
            Link::Alone => {
                self.forget_server_presence(server_id);
                self.forget_notifications(server_id, None, None).await;
                self.drop_server_media(server_id).await;
            }
        }
    }

    /// Deletes the pictures kept here that were made for or used by a server
    /// that's gone, and their rows. A failure is logged and reported.
    async fn drop_server_media(&self, server_id: &str) {
        let dropped = async {
            let ids = self.node()?.media_of_server(server_id).await?;
            self.delete_media(&ids).await
        };
        if dropped.await.is_err() {
            tracing::warn!(server = %server_id, "couldn't delete a deleted server's pictures");
            crate::reports::server_error("server_media_drop", Some("cluster::calls"));
        }
    }

    /// Drops notification settings that no longer point anywhere. Losing them
    /// only leaves a few unused rows, so a failure is just logged.
    pub async fn forget_notifications(&self, server_id: &str, channel_id: Option<&str>, account_id: Option<&str>) {
        let forgotten = match &self.link {
            Link::Shard(link) => {
                let request = cpb::ForgetNotificationsRequest {
                    server_id: server_id.to_string(),
                    channel_id: channel_id.unwrap_or_default().to_string(),
                    account_id: account_id.unwrap_or_default().to_string(),
                };
                link.directory().forget_notifications(request).await.map(|_| ()).map_err(Error::from)
            }
            _ => async { self.node()?.forget_notification_settings(server_id, channel_id, account_id).await }.await,
        };
        if let Err(err) = forgotten {
            tracing::warn!(server = %server_id, error = %err, "couldn't forget notification settings");
        }
    }

    /// Checks a picture link about to be set, for `server_id`'s icon, emoji
    /// or webhook or else for the account. A link to one of this instance's
    /// uploads must be to one the caller uploaded, for this purpose (and, if
    /// it was uploaded for a server, for this one), and stored; its id comes
    /// back so `keep_picture` can mark it used once the change is saved. Any
    /// other link passes as it is.
    pub async fn check_picture(
        &self,
        account_id: &str,
        purpose: pb::MediaPurpose,
        url: &str,
        server_id: Option<&str>,
    ) -> Result<Option<String>> {
        Ok(self.check_upload(account_id, purpose, url, server_id).await?.map(|upload| upload.id))
    }

    /// [`check_picture`](Self::check_picture), telling the upload's size and type too.
    pub async fn check_upload(
        &self,
        account_id: &str,
        purpose: pb::MediaPurpose,
        url: &str,
        server_id: Option<&str>,
    ) -> Result<Option<pb::Media>> {
        let Some(id) = media::id_in_url(url) else { return Ok(None) };
        if let Link::Shard(link) = &self.link {
            let request = cpb::CheckPictureRequest {
                account_id: account_id.to_string(),
                purpose: purpose as i32,
                url: url.to_string(),
                server_id: server_id.unwrap_or_default().to_string(),
            };
            let checked = link.ask(request, |mut d, r| async move { d.check_picture(r).await }).await?;
            return Ok(Some(checked).filter(|c| !c.media_id.is_empty()).map(|c| pb::Media {
                id: c.media_id,
                url: url.to_string(),
                content_type: c.content_type,
                size: c.size,
            }));
        }
        let Some(row) = self.node()?.media(&id).await? else { return Ok(None) };
        if row.account_id != account_id || row.purpose != purpose {
            return Err(Error::denied("upload that picture yourself to use it here"));
        }
        // A picture made for or used by a server is only ever that server's:
        // it may be kept in that server's region, and it's deleted when that
        // server replaces it or stops using it, so another server can't
        // share it.
        if row.server_id.is_some() && row.server_id.as_deref() != server_id {
            return Err(Error::denied("that picture belongs to another server; upload it here"));
        }
        if !row.stored {
            return Err(Error::FailedPrecondition("that picture hasn't finished uploading".into()));
        }
        Ok(Some(pb::Media { id, url: url.to_string(), content_type: row.content_type, size: row.size }))
    }

    /// Marks a checked upload as used. A failure only means it may be swept
    /// later, so it's logged.
    pub async fn keep_picture(&self, id: Option<&str>, server_id: Option<&str>) {
        let Some(id) = id else { return };
        let kept = match &self.link {
            Link::Shard(link) => {
                let request = cpb::KeepPictureRequest {
                    media_id: id.to_string(),
                    server_id: server_id.unwrap_or_default().into(),
                };
                link.directory().keep_picture(request).await.map(|_| ()).map_err(Error::from)
            }
            _ => async { self.node()?.use_media(id, server_id).await }.await,
        };
        if let Err(err) = kept {
            tracing::warn!(media = %id, error = %err, "couldn't mark a picture as used");
        }
    }

    /// Deletes the picture a change replaced, if it was one of this
    /// instance's uploads and belonged to what changed: the account's own
    /// avatar or banner, or the server's icon, emoji or webhook pictures.
    pub async fn drop_picture(&self, old_url: &str, new_url: &str, owner: PictureOwner<'_>) {
        if old_url == new_url {
            return;
        }
        let Some(id) = media::id_in_url(old_url) else { return };
        if let (Link::Shard(link), PictureOwner::Server(server_id)) = (&self.link, owner) {
            let request = cpb::DropPictureRequest {
                old_url: old_url.to_string(),
                new_url: new_url.to_string(),
                server_id: server_id.to_string(),
            };
            if link.directory().drop_picture(request).await.is_err() {
                tracing::warn!(media = %id, "couldn't delete a replaced picture");
                // Its copy here goes anyway unless the server still uses it;
                // the directory's row is left to the sweeps.
                if !matches!(crate::cluster::pictures::uses(self, server_id, &id).await, Ok(true)) {
                    crate::cluster::pictures::drop(self, server_id, &id).await;
                }
                return;
            }
            // The directory deletes it only if it was the server's, and only
            // the server's pictures are ever kept here.
            crate::cluster::pictures::drop(self, server_id, &id).await;
            return;
        }
        let row = match async { self.node()?.media(&id).await }.await {
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
                matches!(
                    row.purpose,
                    pb::MediaPurpose::ServerIcon
                        | pb::MediaPurpose::Emoji
                        | pb::MediaPurpose::Avatar
                        | pb::MediaPurpose::Attachment
                ) && row.server_id.as_deref() == Some(server_id)
            }
        };
        if belongs && let Err(err) = self.delete_media(&[id]).await {
            tracing::warn!(media = %row.id, error = %err, "couldn't delete a replaced picture");
        }
    }

    /// How many servers an account owns.
    pub async fn owned_count(&self, account_id: &str) -> Result<i64> {
        match &self.link {
            Link::Shard(link) => {
                let request = cpb::CountOwnedServersRequest { account_id: account_id.to_string() };
                Ok(link.ask(request, |mut d, r| async move { d.count_owned_servers(r).await }).await?.count)
            }
            _ => Ok(self.index.owned_count(account_id)),
        }
    }

    /// An agent that can be added to servers, by username: none if there's
    /// no such agent or it's turned off.
    pub async fn find_agent(&self, username: &str) -> Result<Option<FoundAgent>> {
        match &self.link {
            Link::Shard(link) => {
                let request = cpb::FindAgentRequest { username: username.to_string() };
                let found = link.ask(request, |mut d, r| async move { d.find_agent(r).await }).await?;
                Ok(found.agent.map(|agent| FoundAgent {
                    account: super::account_from_pb(agent),
                    owner_id: found.owner_id,
                    public: found.public,
                }))
            }
            _ => Ok(self
                .node()?
                .agent(None, Some(username))
                .await?
                .filter(|row| !row.account.disabled)
                .map(|row| FoundAgent { account: row.account, owner_id: row.owner_id, public: row.public })),
        }
    }

    /// Which of the emoji someone wrote from their other servers they may
    /// use: ones of a server they're a member of that it still has, with a
    /// picture that's one of that server's emoji here. They come back as the
    /// server keeps them (only the id sent is used): the stored name, the
    /// link rebuilt at this instance's public address, and animated from the
    /// picture itself.
    pub async fn check_emojis(&self, account_id: &str, emojis: Vec<pb::Emoji>) -> Result<Vec<pb::Emoji>> {
        if emojis.is_empty() {
            return Ok(vec![]);
        }
        if let Link::Shard(link) = &self.link {
            let request = cpb::CheckEmojisRequest { account_id: account_id.to_string(), emojis };
            return Ok(link.ask(request, |mut d, r| async move { d.check_emojis(r).await }).await?.emojis);
        }
        // The ids asked for, by server, in the order they came.
        let mut order: Vec<String> = Vec::new();
        let mut by_server: Vec<(String, Vec<String>)> = Vec::new();
        for emoji in emojis.into_iter().take(MAX_OUTSIDE_EMOJIS) {
            if !emoji_id_ok(&emoji.id)
                || order.contains(&emoji.id)
                || !self.index.is_member(account_id, &emoji.server_id)
            {
                continue;
            }
            order.push(emoji.id.clone());
            match by_server.iter_mut().find(|(server_id, _)| *server_id == emoji.server_id) {
                Some((_, ids)) => ids.push(emoji.id),
                None => by_server.push((emoji.server_id, vec![emoji.id])),
            }
        }
        let node = self.node()?;
        let base = self.settings().public_url.clone();
        let mut found: HashMap<String, pb::Emoji> = HashMap::new();
        for (server_id, ids) in by_server {
            // A server that can't be asked leaves its emoji as names.
            let Ok(stored) = self.server_emojis(&server_id, ids).await else { continue };
            for emoji in stored {
                let Some(media_id) = media::id_in_url(&emoji.url) else { continue };
                let Some(row) = node.media(&media_id).await? else { continue };
                if row.purpose != pb::MediaPurpose::Emoji
                    || !row.stored
                    || !row.used
                    || row.server_id.as_deref() != Some(server_id.as_str())
                {
                    continue;
                }
                found.insert(
                    emoji.id.clone(),
                    pb::Emoji {
                        id: emoji.id,
                        server_id: server_id.clone(),
                        name: emoji.name,
                        url: format!("{base}/media/{media_id}"),
                        animated: row.content_type == "image/gif",
                        ..Default::default()
                    },
                );
            }
        }
        Ok(order.iter().filter_map(|id| found.remove(id)).collect())
    }

    /// A server's emoji with these ids, asked of the shard holding it.
    async fn server_emojis(&self, server_id: &str, ids: Vec<String>) -> Result<Vec<pb::Emoji>> {
        let Link::Directory(shards) = &self.link else {
            return shard::server_emojis(&self.servers, server_id, &ids).await;
        };
        let shard_id = self.index.placement(server_id).ok_or(Error::NotFound("server"))?;
        let request = cpb::ServerEmojisRequest { server_id: server_id.to_string(), ids };
        Ok(shards.client(&shard_id)?.server_emojis(request).await?.into_inner().emojis)
    }

    /// The regions this instance keeps servers in, the home region first;
    /// one or none when there's nothing to choose.
    pub fn regions(&self) -> Vec<pb::Region> {
        match &self.link {
            Link::Directory(shards) => shards.regions(),
            _ if self.config.cluster.region.is_empty() => vec![],
            _ => {
                let cluster = &self.config.cluster;
                let name = super::region_name(&cluster.region, cluster.region_name.as_deref());
                vec![pb::Region { id: cluster.region.clone(), name, home: true }]
            }
        }
    }

    /// The caller's device and the live devices of `account_ids`, for
    /// checking a secure channel's commits. Devices live where direct
    /// messages do, on the directory.
    pub async fn secure_devices(&self, token_hash: &str, account_ids: &[String]) -> Result<SecureDevices> {
        match &self.link {
            Link::Shard(link) => {
                let request =
                    cpb::SecureDevicesRequest { token_hash: token_hash.to_string(), account_ids: account_ids.to_vec() };
                let found = link.ask(request, |mut d, r| async move { d.secure_devices(r).await }).await?;
                Ok(SecureDevices {
                    caller: Some(found.caller_device_id).filter(|id| !id.is_empty()),
                    devices: found.devices.into_iter().map(|d| (d.id, d.account_id)).collect(),
                })
            }
            _ => {
                let node = self.node()?;
                let dms = self.dms()?;
                let caller = match node.session_id(token_hash).await? {
                    Some(session_id) => dms.session_device(&session_id).await?.map(|device| device.id),
                    None => None,
                };
                let ids: Vec<&str> = account_ids.iter().map(String::as_str).collect();
                let live = node.live_session_ids(Some(&ids)).await?;
                let mut devices = dms.devices_of(&ids).await?;
                devices.retain(|device| live.contains(&device.session_id));
                Ok(SecureDevices { caller, devices: devices.into_iter().map(|d| (d.id, d.account_id)).collect() })
            }
        }
    }

    // ─────────────── Asked of the shards ───────────────

    /// Makes a new server in `region` (empty for the home region): here, or
    /// on the shard there holding the fewest.
    pub async fn create_server(&self, owner: &pb::User, new: NewServer, region: &str) -> Result<pb::Server> {
        if !region.is_empty() {
            super::check_region(region).map_err(|err| Error::invalid(format!("region {err}")))?;
        }
        let Link::Directory(shards) = &self.link else {
            if !region.is_empty() && region != self.config.cluster.region {
                return Err(Error::invalid(format!("this instance has no region {region:?}")));
            }
            let server = self.servers.create(owner, new).await?;
            self.index.insert(server.clone(), vec![owner.id.clone()], vec![], None);
            return Ok(server);
        };
        let (shard_id, mut client) = shards.emptiest_in(region, &self.index.shard_sizes())?;
        let request = cpb::CreateServerRequest {
            owner: Some(owner.clone()),
            name: new.name,
            description: new.description,
            icon_url: new.icon_url,
            discoverable: new.discoverable,
        };
        let server = client
            .create_server(request)
            .await?
            .into_inner()
            .server
            .ok_or_else(|| Error::internal("the shard didn't say what it made"))?;
        self.node()?.place(&server.id, Some(&shard_id)).await?;
        self.index.insert(server.clone(), vec![owner.id.clone()], vec![], Some(&shard_id));
        Ok(server)
    }

    /// Copies someone's new profile into every server they're in.
    pub async fn update_user(&self, user: &pb::User, server_ids: Vec<String>) {
        let Link::Directory(shards) = &self.link else {
            return shard::update_user(&self.servers, user, &server_ids).await;
        };
        for (shard_id, server_ids) in self.index.by_shard(&server_ids) {
            let request = cpb::UpdateUserRequest { user: Some(user.clone()), server_ids };
            let updated = match shards.client(&shard_id) {
                Ok(mut client) => client.update_user(request).await.map(|_| ()).map_err(Error::from),
                Err(err) => Err(err),
            };
            if let Err(err) = updated {
                tracing::warn!(shard = %shard_id, error = %err, "couldn't update a member's profile");
            }
        }
    }

    /// Takes a deleted account out of every server it was ever in, leaving
    /// `placeholder` as its name on what it wrote. Every shard must be up, so
    /// none keeps the account's name.
    pub async fn forget_account(&self, account_id: &str, placeholder: &pb::User) -> Result<()> {
        let left = match &self.link {
            Link::Directory(shards) => {
                let all = shards.all();
                if let Some((id, _)) = all.iter().find(|(_, client)| client.is_none()) {
                    return Err(Error::Unavailable(format!(
                        "shard {id} is down, and it may hold servers you were in; try again once it's back"
                    )));
                }
                let mut left = Vec::new();
                for (_, client) in all {
                    let Some(mut client) = client else { continue };
                    let request = cpb::ForgetAccountRequest {
                        account_id: account_id.to_string(),
                        placeholder: Some(placeholder.clone()),
                    };
                    left.extend(client.forget_account(request).await?.into_inner().left_server_ids);
                }
                left
            }
            _ => shard::forget_account(&self.servers, account_id, placeholder).await?,
        };
        for server_id in left {
            self.index.leave(account_id, &server_id);
        }
        Ok(())
    }

    /// An account's part of a data export from every server: pieces of JSON,
    /// the first of each server's starting its object.
    pub fn export_account(self: &Arc<Self>, account_id: &str) -> mpsc::Receiver<Result<cpb::ExportAccountResponse>> {
        let Link::Directory(shards) = &self.link else {
            return shard::export_account(self.clone(), account_id.to_string());
        };
        let all = shards.all();
        let account_id = account_id.to_string();
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            for (id, client) in all {
                let Some(mut client) = client else {
                    let down = format!("shard {id} is down, so the export would be missing servers; try again soon");
                    let _ = tx.send(Err(Error::Unavailable(down))).await;
                    return;
                };
                let mut stream =
                    match client.export_account(cpb::ExportAccountRequest { account_id: account_id.clone() }).await {
                        Ok(stream) => stream.into_inner(),
                        Err(status) => {
                            let _ = tx.send(Err(status.into())).await;
                            return;
                        }
                    };
                loop {
                    let piece = match stream.message().await {
                        Ok(Some(piece)) => Ok(piece),
                        Ok(None) => break,
                        Err(status) => Err(Error::from(status)),
                    };
                    let failed = piece.is_err();
                    if tx.send(piece).await.is_err() || failed {
                        return;
                    }
                }
            }
        });
        rx
    }

    /// What admins see about servers (all of them when `ids` is empty). A
    /// shard that's down leaves its servers out.
    pub async fn describe_servers(&self, ids: &[String]) -> Result<Vec<cpb::ServerDescription>> {
        let Link::Directory(shards) = &self.link else {
            return shard::describe_servers(&self.servers, ids).await;
        };
        let mut described = Vec::new();
        let mut grouped: HashMap<String, Vec<String>> = self.index.by_shard(ids);
        if ids.is_empty() {
            grouped.values_mut().for_each(Vec::clear);
        }
        for (shard_id, server_ids) in grouped {
            let found = match shards.client(&shard_id) {
                Ok(mut client) => {
                    client.describe_servers(cpb::DescribeServersRequest { server_ids }).await.map_err(Error::from)
                }
                Err(err) => Err(err),
            };
            match found {
                Ok(found) => described.extend(found.into_inner().servers),
                Err(err) => tracing::warn!(shard = %shard_id, error = %err, "left a shard's servers out"),
            }
        }
        described.sort_by(|a, b| {
            let id = |d: &cpb::ServerDescription| d.server.as_ref().map(|s| s.id.clone()).unwrap_or_default();
            id(a).cmp(&id(b))
        });
        Ok(described)
    }

    /// Where an invite leads, asked of the shard holding its server.
    pub async fn describe_invite(&self, code: &str) -> Result<pb::GetInviteResponse> {
        let server_id = self.index.invite(code).ok_or(Error::NotFound("invite"))?;
        let Link::Directory(shards) = &self.link else {
            return shard::describe_invite(&self.servers, &server_id, code).await;
        };
        let shard_id = self.index.placement(&server_id).ok_or(Error::NotFound("invite"))?;
        let request = cpb::DescribeInviteRequest { server_id, code: code.to_string() };
        shards.client(&shard_id)?.describe_invite(request).await?.into_inner().invite.ok_or(Error::NotFound("invite"))
    }

    /// Whether a server has a channel.
    pub async fn channel_exists(&self, server_id: &str, channel_id: &str) -> Result<bool> {
        let Link::Directory(shards) = &self.link else {
            return shard::channel_exists(&self.servers, server_id, channel_id).await;
        };
        let shard_id = self.index.placement(server_id).ok_or(Error::NotFound("server"))?;
        let request =
            cpb::ChannelExistsRequest { server_id: server_id.to_string(), channel_id: channel_id.to_string() };
        Ok(shards.client(&shard_id)?.channel_exists(request).await?.into_inner().exists)
    }

    // ─────────────── Between shared channels' servers ───────────────

    /// A call between the two ends of a shared channel, answered where the
    /// server it's for is kept: here, on its shard through the directory
    /// (shards don't know each other), or on another instance.
    pub async fn shared(self: &Arc<Self>, call: cpb::SharedCall) -> Result<cpb::SharedReply> {
        if self.servers.holds(&call.server_id) {
            return crate::api::shared_call(self, call).await;
        }
        // A server on another instance ("<id>@<instance>"): the part that
        // keeps the instance's key calls it; a shard passes it there.
        if call.server_id.contains('@') && !matches!(self.link, Link::Shard(_)) {
            return crate::federation::shared(self, call).await;
        }
        match &self.link {
            Link::Shard(link) => {
                let request = cpb::PassSharedRequest { call: Some(call) };
                Ok(link.directory().pass_shared(request).await?.into_inner().reply.unwrap_or_default())
            }
            Link::Directory(shards) => {
                let shard_id = self.index.placement(&call.server_id).ok_or(Error::NotFound("server"))?;
                let request = cpb::SharedRequest { call: Some(call) };
                Ok(shards.client(&shard_id)?.shared(request).await?.into_inner().reply.unwrap_or_default())
            }
            Link::Alone => Err(Error::NotFound("server")),
        }
    }
}

/// Most emoji from other servers one message keeps.
pub const MAX_OUTSIDE_EMOJIS: usize = 50;

/// An emoji's id as messages write it: 10 to 32 letters and digits.
fn emoji_id_ok(id: &str) -> bool {
    (10..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}
