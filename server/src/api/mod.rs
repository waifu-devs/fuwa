//! The gRPC services. One `Api` value implements all of them over the shared app.

mod account;
mod admin;
mod agents;
mod auth;
mod automod;
mod calls;
mod channels;
mod commands;
mod dms;
mod emoji;
mod events;
mod friends;
mod gifs;
mod invites;
mod join;
mod live_tiles;
mod media;
mod messages;
mod node;
mod pins;
mod polls;
mod presence;
mod providers;
mod roles;
mod search;
mod secure;
mod servers;
mod shared;
mod sso;
mod threads;
mod webhooks;

pub(crate) use account::export_server;
pub use calls::{hang_up_server, spawn_voice_guard, spawn_voice_sweeper};
pub use live_tiles::{LIVE_TILE_PUBLISH_MS, LIVE_TILE_UPDATES_PER_MINUTE};
pub use media::PictureOwner;
pub(crate) use polls::{close_due as close_due_polls, forget_voter as forget_poll_voter};
pub use search::spawn_search_indexer;
pub use secure::MAX_SECURE_MEMBERS;
pub use shared::{
    arrived as shared_arrived, file_for as shared_file_for, returned as shared_returned, shared_call,
    spawn_shared_fanout, undo as shared_undo,
};
pub use sso::note_lapses;
pub use webhooks::{WebhookPost, end_webhook_tile, execute_webhook, set_webhook_tile, verify_webhook};

use std::sync::Arc;

use tonic::metadata::MetadataMap;

use crate::app::App;
use crate::auth::{Caller, Viewer};
use crate::db::query_all;
use crate::error::{Error, Result};
use crate::node::Account;
use crate::pb;
use crate::permissions::Access;
use crate::servers::{self as store, ServerDb, USER_COLUMNS};

#[derive(Clone)]
pub struct Api {
    app: Arc<App>,
}

impl Api {
    pub fn new(app: Arc<App>) -> Self {
        Self { app }
    }

    async fn viewer(&self, metadata: &MetadataMap) -> Result<Viewer> {
        self.app.authenticate(metadata).await
    }

    async fn account(&self, metadata: &MetadataMap) -> Result<Account> {
        Ok(self.viewer(metadata).await?.account()?.clone())
    }

    /// The signed-in account and the session it called with.
    async fn caller(&self, metadata: &MetadataMap) -> Result<Caller> {
        self.viewer(metadata).await?.caller()
    }

    /// Drops notification settings that no longer point anywhere. Losing them
    /// only leaves a few unused rows, so a failure is just logged.
    async fn forget_notifications(&self, server_id: &str, channel_id: Option<&str>, account_id: Option<&str>) {
        self.app.forget_notifications(server_id, channel_id, account_id).await
    }

    /// The server, the caller's membership in it, and what they can do there.
    async fn membership(&self, account: &Account, server_id: &str) -> Result<Seat> {
        let sdb = self.app.servers.get(server_id).await?;
        let (member, access) =
            sdb.member_access(&account.id).await?.ok_or_else(|| Error::denied("join this server first"))?;
        Ok(Seat { sdb, member, access })
    }

    /// Like `membership`, for callers who need `permission` server-wide.
    async fn with(&self, account: &Account, server_id: &str, permission: pb::Permission) -> Result<Seat> {
        let seat = self.membership(account, server_id).await?;
        seat.access.require(permission)?;
        Ok(seat)
    }
}

/// A caller's place in a server, as of their request.
struct Seat {
    sdb: Arc<ServerDb>,
    member: pb::Member,
    access: Access,
}

/// Trims `value` and checks its length in characters.
fn text(field: &str, value: &str, min: usize, max: usize) -> Result<String> {
    let value = value.trim();
    let length = value.chars().count();
    if length < min || length > max {
        return Err(Error::invalid(if min == 0 {
            format!("{field} can be at most {max} characters")
        } else {
            format!("{field} must be {min} to {max} characters")
        }));
    }
    Ok(value.to_string())
}

/// A profile effect's id: lowercase letters, digits and dashes, up to 32, or
/// empty for none. Ids are the apps' to name (docs/profile-effects.md), so the
/// server checks only the shape and apps show nothing for one they don't know.
fn effect_id(value: &str) -> Result<String> {
    let value = value.trim();
    let shaped = value.len() <= 32
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !value.starts_with('-');
    if !shaped {
        return Err(Error::invalid("effect is an id of lowercase letters, digits and dashes, up to 32"));
    }
    Ok(value.to_string())
}

/// An optional http(s) URL, such as an icon or avatar.
fn url(field: &str, value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.len() > 2048 || !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err(Error::invalid(format!("{field} must be an http(s) URL of at most 2048 characters")));
    }
    Ok(value.to_string())
}

/// The users with these ids, as the server last saw them. Ids it never saw are skipped.
async fn users(conn: &turso::Connection, ids: &[&str]) -> Result<Vec<pb::User>> {
    let mut ids = ids.to_vec();
    ids.retain(|id| !id.is_empty());
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    query_all(
        conn,
        &format!("SELECT {USER_COLUMNS} FROM users WHERE id IN ({placeholders})"),
        ids.iter().map(|id| turso::Value::from(*id)).collect::<Vec<_>>(),
        store::user_row,
    )
    .await
}

/// Turns a crate result into a tonic response.
fn respond<T>(result: Result<T>) -> std::result::Result<tonic::Response<T>, tonic::Status> {
    result.map(tonic::Response::new).map_err(Into::into)
}
