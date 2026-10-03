use std::collections::BTreeSet;

use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::error::{Error, Result};
use crate::id::{now_ms, parse_id, timestamp};
use crate::pb::{self, Permission, invite_service_server::InviteService};
use crate::servers::{self as store, Audit};

/// The most people one invite can be for, short of no limit, as Discord has it.
const MAX_USES: i32 = 100;

/// The longest an invite can last, short of forever: a year.
const MAX_AGE_SECONDS: i32 = 365 * 24 * 60 * 60;

/// Letters and digits that read the same in any font.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
const CODE_LENGTH: usize = 10;

/// A fresh invite code, random enough that guessing one is hopeless.
fn new_code() -> String {
    let mut bytes = [0u8; CODE_LENGTH];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    // 256 isn't a multiple of the alphabet's size, so the first few letters
    // come up a little more often; at this length that costs nothing.
    bytes.iter().map(|b| CODE_ALPHABET[usize::from(*b) % CODE_ALPHABET.len()] as char).collect()
}

/// Whether a code could be one this instance made; others aren't looked up.
fn plausible(code: &str) -> bool {
    (1..=32).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric())
}

#[tonic::async_trait]
impl InviteService for Api {
    async fn create_invite(
        &self,
        request: Request<pb::CreateInviteRequest>,
    ) -> Result<Response<pb::CreateInviteResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                if !(0..=MAX_USES).contains(&req.max_uses) {
                    return Err(Error::invalid(format!("an invite is for 1 to {MAX_USES} people, or 0 for no limit")));
                }
                if !(0..=MAX_AGE_SECONDS).contains(&req.max_age_seconds) {
                    return Err(Error::invalid("an invite lasts up to a year, or 0 for forever"));
                }
                let seat = self.membership(&account, &req.server_id).await?;
                let channel_id = match req.channel_id.as_str() {
                    "" => {
                        seat.access.require(Permission::CreateInvite)?;
                        None
                    }
                    id => {
                        let id = parse_id("channel_id", id)?;
                        seat.access.require_in(&id, Permission::CreateInvite)?;
                        Some(id)
                    }
                };
                let sdb = seat.sdb;
                let now = now_ms();
                let code = new_code();
                let expires_at = (req.max_age_seconds > 0).then(|| now + i64::from(req.max_age_seconds) * 1000);
                let (invite, swept) = sdb
                    .write(&account.id, async |conn, _events| {
                        let mut entry = Audit::new(pb::AuditAction::InviteCreate, code.as_str());
                        if let Some(channel_id) = channel_id.as_deref() {
                            let channel = store::load_channel(conn, &sdb.id, channel_id)
                                .await?
                                .ok_or(Error::NotFound("channel"))?;
                            if !matches!(
                                pb::ChannelType::try_from(channel.r#type),
                                Ok(pb::ChannelType::Text | pb::ChannelType::Announcement)
                            ) {
                                return Err(Error::invalid("invites open a text or announcement channel"));
                            }
                            entry = entry.channel(channel.name);
                        }
                        let swept = store::sweep_invites(conn, now).await?;
                        conn.execute(
                            "INSERT INTO invites (code, channel_id, inviter_id, max_uses, expires_at, created_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            (code.as_str(), channel_id.as_deref(), account.id.as_str(), req.max_uses, expires_at, now),
                        )
                        .await?;
                        let entry = entry.change("max_uses", "", req.max_uses).change(
                            "expires_at",
                            "",
                            expires_at.map(|t| t.to_string()).unwrap_or_default(),
                        );
                        store::audit(conn, &account.id, entry).await?;
                        Ok((
                            pb::Invite {
                                code: code.clone(),
                                server_id: sdb.id.clone(),
                                channel_id: channel_id.clone().unwrap_or_default(),
                                inviter_id: account.id.clone(),
                                max_uses: req.max_uses,
                                uses: 0,
                                expires_at: expires_at.map(timestamp),
                                created_at: Some(timestamp(now)),
                            },
                            swept,
                        ))
                    })
                    .await?;
                self.app.index_invite(&sdb.id, &invite.code, true).await;
                for code in swept {
                    self.app.index_invite(&sdb.id, &code, false).await;
                }
                Ok(pb::CreateInviteResponse { invite: Some(invite) })
            }
            .await,
        )
    }

    async fn list_invites(
        &self,
        request: Request<pb::ListInvitesRequest>,
    ) -> Result<Response<pb::ListInvitesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let seat = self.membership(&account, &request.get_ref().server_id).await?;
                let all = seat.access.has(Permission::ManageServer);
                let conn = seat.sdb.read()?;
                let now = now_ms();
                let invites: Vec<pb::Invite> = store::load_invites(&conn, &seat.sdb.id)
                    .await?
                    .into_iter()
                    .filter(|i| store::invite_works(i, now) && (all || i.inviter_id == account.id))
                    .collect();
                let mut inviters = Vec::new();
                for id in invites.iter().map(|i| i.inviter_id.as_str()).collect::<BTreeSet<_>>() {
                    inviters.extend(store::user(&conn, id).await?);
                }
                Ok(pb::ListInvitesResponse { invites, inviters })
            }
            .await,
        )
    }

    async fn delete_invite(
        &self,
        request: Request<pb::DeleteInviteRequest>,
    ) -> Result<Response<pb::DeleteInviteResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let seat = self.membership(&account, &req.server_id).await?;
                seat.access.require_not_timed_out()?;
                let manager = seat.access.has(Permission::ManageServer);
                let sdb = seat.sdb;
                sdb.write(&account.id, async |conn, _events| {
                    let invite =
                        store::load_invite(conn, &sdb.id, &req.code).await?.ok_or(Error::NotFound("invite"))?;
                    if invite.inviter_id != account.id && !manager {
                        return Err(crate::permissions::missing(Permission::ManageServer));
                    }
                    conn.execute("DELETE FROM invites WHERE code = ?1", [invite.code.as_str()]).await?;
                    let mut entry = Audit::new(pb::AuditAction::InviteDelete, invite.code.as_str());
                    if !invite.channel_id.is_empty()
                        && let Some(channel) = store::load_channel(conn, &sdb.id, &invite.channel_id).await?
                    {
                        entry = entry.channel(channel.name);
                    }
                    store::audit(conn, &account.id, entry.change("uses", invite.uses, "")).await
                })
                .await?;
                self.app.index_invite(&sdb.id, &req.code, false).await;
                Ok(pb::DeleteInviteResponse {})
            }
            .await,
        )
    }

    async fn get_invite(
        &self,
        request: Request<pb::GetInviteRequest>,
    ) -> Result<Response<pb::GetInviteResponse>, Status> {
        respond(
            async {
                let code = request.get_ref().code.trim();
                if !plausible(code) {
                    return Err(Error::NotFound("invite"));
                }
                self.app.describe_invite(code).await
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_fresh_and_readable() {
        let a = new_code();
        assert_eq!(a.len(), CODE_LENGTH);
        assert!(a.bytes().all(|b| CODE_ALPHABET.contains(&b)));
        assert!(plausible(&a));
        assert_ne!(a, new_code());
        assert!(!plausible(""));
        assert!(!plausible("../etc"));
    }
}
