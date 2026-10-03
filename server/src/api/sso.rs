//! `SsoService`: one server's single sign-on (see `crate::sso`). The
//! provider and whether it's required live in the server row; who signed in
//! lives in `sso_identities`, members or not yet; sign-ins on their way in
//! `sso_sign_ins`. Changing the provider writes an audit entry (never its
//! secret) and ServerUpdated, which makes every member's stream work out
//! again what they can see.

use tonic::{Request, Response, Status};

use super::{Api, Seat, respond};
use crate::db::{is_unique_violation, query_one};
use crate::error::{Error, Result};
use crate::id::now_ms;
use crate::node::Account;
use crate::pb::{self, Permission, sso_service_server::SsoService};
use crate::servers::{self as store, Audit, Payload, ServerDb, ServerSso};
use crate::sso::{self, Endpoints, Protocol, Provider, Scope};

/// The longest a sign-in can last before members sign in again: a year.
const MAX_RECHECK_DAYS: i32 = 365;

impl Api {
    fn server_endpoints(&self, server_id: &str) -> Endpoints {
        Endpoints::new(&self.app.settings().public_url, &Scope::Server(server_id.to_string()))
    }

    async fn server_sso(&self, sdb: &ServerDb) -> Result<pb::ServerSso> {
        let conn = sdb.read()?;
        let sso = store::load_sso(&conn).await?;
        let mut signed_in = 0;
        if sso.provider.is_set() {
            let since =
                if sso.recheck_days > 0 { now_ms() - i64::from(sso.recheck_days) * 86_400_000 } else { i64::MIN };
            signed_in = query_one(
                &conn,
                "SELECT count(*) FROM sso_identities JOIN members ON members.user_id = sso_identities.user_id
                 WHERE sso_identities.signed_in_at > ?1",
                [since],
                |r| r.get::<i64>(0),
            )
            .await?
            .unwrap_or(0);
        }
        Ok(pb::ServerSso {
            provider: Some(sso.provider.to_pb()),
            required: sso.required,
            recheck_days: sso.recheck_days,
            service_provider: Some(self.server_endpoints(&sdb.id).to_pb()),
            signed_in_members: signed_in,
        })
    }

    /// Who may start signing in to a server: anyone who could get in (it's
    /// in Browse, or they hold an invite) and its members.
    async fn may_start(&self, account: &Account, sdb: &ServerDb, invite_code: &str) -> Result<()> {
        if account.kind == pb::AccountKind::Agent {
            return Err(Error::FailedPrecondition("agents don't sign in through single sign-on".into()));
        }
        let conn = sdb.read()?;
        if store::member(&conn, &sdb.id, &account.id).await?.is_some() || store::load_server(&conn).await?.discoverable
        {
            return Ok(());
        }
        let code = invite_code.trim();
        if !code.is_empty()
            && store::load_invite(&conn, &sdb.id, code)
                .await?
                .is_some_and(|invite| store::invite_works(&invite, now_ms()))
        {
            return Ok(());
        }
        Err(Error::NotFound("server"))
    }
}

/// Who signs members in decides who gets into the server, so only its owner
/// picks the provider, not everyone who can manage it.
fn only_the_owner(owner: bool) -> Result<()> {
    if owner { Ok(()) } else { Err(Error::denied("only the server's owner can set up single sign-on")) }
}

/// The audit entry for a change to single sign-on: never the secret.
fn audit(before: &ServerSso, after: &ServerSso) -> Audit {
    let describe = |p: &Provider| match p.protocol {
        Protocol::None => String::new(),
        Protocol::Oidc => format!("{} (OIDC {}, client {})", p.name, p.oidc_issuer, p.oidc_client_id),
        Protocol::Saml => format!(
            "{} (SAML {}, signs in at {}, certificates SHA-256 {})",
            p.name,
            p.saml_entity_id,
            p.saml_sso_url,
            p.fingerprints().join(" ")
        ),
    };
    let entry = Audit::new(pb::AuditAction::ServerUpdate, "")
        .change("sso_provider", describe(&before.provider), describe(&after.provider))
        .change("sso_required", before.required, after.required)
        .change("sso_recheck_days", before.recheck_days, after.recheck_days);
    let domains = |p: &Provider| p.email_domains.join(", ");
    let secret_changed = before.provider.oidc_client_secret != after.provider.oidc_client_secret
        && before.provider.trust_key() == after.provider.trust_key()
        && after.provider.is_set();
    let entry = entry.change("sso_email_domains", domains(&before.provider), domains(&after.provider));
    if secret_changed { entry.change("sso_client_secret", "", "changed") } else { entry }
}

#[tonic::async_trait]
impl SsoService for Api {
    async fn get_server_sso(
        &self,
        request: Request<pb::GetServerSsoRequest>,
    ) -> Result<Response<pb::GetServerSsoResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let Seat { sdb, access, .. } =
                    self.with(&account, &request.get_ref().server_id, Permission::ManageServer).await?;
                only_the_owner(access.owner)?;
                let conn = sdb.read()?;
                let mine = query_one(
                    &conn,
                    "SELECT subject, email, name, signed_in_at FROM sso_identities WHERE user_id = ?1",
                    [account.id.as_str()],
                    |r| {
                        Ok(pb::SsoIdentity {
                            subject: r.get(0)?,
                            email: r.get(1)?,
                            name: r.get(2)?,
                            signed_in_at: Some(crate::id::timestamp(r.get(3)?)),
                        })
                    },
                )
                .await?;
                Ok(pb::GetServerSsoResponse { sso: Some(self.server_sso(&sdb).await?), mine })
            }
            .await,
        )
    }

    async fn update_server_sso(
        &self,
        request: Request<pb::UpdateServerSsoRequest>,
    ) -> Result<Response<pb::UpdateServerSsoResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageServer).await?;
                only_the_owner(access.owner)?;
                if req.recheck_days.is_some_and(|days| !(0..=MAX_RECHECK_DAYS).contains(&days)) {
                    return Err(Error::invalid("members sign in again every 0 (never) to 365 days"));
                }
                let server = sdb
                    .write(&account.id, async |conn, events| {
                        let before = store::load_sso(conn).await?;
                        let provider = if req.remove_provider {
                            Provider::default()
                        } else {
                            match &req.provider {
                                Some(provider) => {
                                    let provider = Provider::from_pb(provider, &before.provider)?;
                                    // Only the instance's own provider may be on this machine.
                                    let url = match provider.protocol {
                                        Protocol::Oidc => provider.oidc_issuer.as_str(),
                                        Protocol::Saml => provider.saml_sso_url.as_str(),
                                        Protocol::None => "https://",
                                    };
                                    if !url.starts_with("https://") {
                                        return Err(Error::invalid("a server's provider must be at an https URL"));
                                    }
                                    if provider.is_set() {
                                        provider.ready()?;
                                    }
                                    provider
                                }
                                None => before.provider.clone(),
                            }
                        };
                        let moved = provider.trust_key() != before.provider.trust_key();
                        if moved {
                            // Sign-ins through another provider don't count for this one.
                            conn.execute("DELETE FROM sso_identities", ()).await?;
                            conn.execute("DELETE FROM sso_sign_ins", ()).await?;
                        }
                        let required = provider.is_set()
                            && match req.required {
                                Some(required) => required,
                                None => before.required && !moved,
                            };
                        let after = ServerSso {
                            provider,
                            required,
                            recheck_days: req.recheck_days.unwrap_or(before.recheck_days),
                        };
                        if after.required && !(before.required && !moved) && !access.owner {
                            let mine = store::sso_signed_in_at(conn, &account.id).await?;
                            if !after.fresh(mine, now_ms()) {
                                return Err(Error::FailedPrecondition(
                                    "sign in through the provider yourself first (Test sign-in), so requiring it doesn't lock you out"
                                        .into(),
                                ));
                            }
                        }
                        conn.execute(
                            "UPDATE server SET sso = ?1, sso_required = ?2, sso_recheck_days = ?3, updated_at = ?4",
                            (after.provider.stored(), after.required, after.recheck_days, now_ms()),
                        )
                        .await?;
                        let entry = audit(&before, &after);
                        if !entry.changes.is_empty() {
                            store::audit(conn, &account.id, entry).await?;
                        }
                        let server = store::load_server(conn).await?;
                        events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                        Ok(server)
                    })
                    .await?;
                self.app.server_changed(&server).await;
                Ok(pb::UpdateServerSsoResponse { sso: Some(self.server_sso(&sdb).await?) })
            }
            .await,
        )
    }

    async fn start_server_sso(
        &self,
        request: Request<pb::StartServerSsoRequest>,
    ) -> Result<Response<pb::StartServerSsoResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.app.servers.get(&req.server_id).await?;
                self.may_start(&account, &sdb, &req.invite_code).await?;
                let provider = store::load_sso(&sdb.read()?).await?.provider;
                if !provider.is_set() {
                    return Err(Error::FailedPrecondition("this server doesn't use single sign-on".into()));
                }
                let endpoints = self.server_endpoints(&sdb.id);
                let settings = self.app.settings();
                let origin =
                    crate::linked::return_origin(&req.return_origin, &settings.public_url, &settings.allowed_origins)?;
                sso::starts().start(&[
                    (&format!("server {}", sdb.id), sso::MAX_STARTS_SERVER),
                    (&format!("server {} account {}", sdb.id, account.id), sso::MAX_STARTS_ACCOUNT),
                ])?;
                let (authorize_url, sign_in) =
                    sso::start(&provider, &endpoints, &origin, &req.secret_hash, &account.id).await?;
                sdb.write(&account.id, async |conn, _| sso::save(conn, &sign_in).await).await?;
                Ok(pb::StartServerSsoResponse { authorize_url, state: sign_in.state })
            }
            .await,
        )
    }

    async fn get_server_sso_sign_in(
        &self,
        request: Request<pb::GetServerSsoSignInRequest>,
    ) -> Result<Response<pb::GetServerSsoSignInResponse>, Status> {
        respond(
            async {
                let req = request.into_inner();
                let sdb = self.app.servers.get(&req.server_id).await?;
                let conn = sdb.read()?;
                let sign_in = sso::load(&conn, req.state.trim()).await?.ok_or(Error::NotFound("sign-in"))?;
                let server = store::load_server(&conn).await?;
                let provider = store::load_sso(&conn).await?.provider;
                Ok(pb::GetServerSsoSignInResponse {
                    return_origin: sign_in.return_origin,
                    server_name: server.name,
                    provider_name: provider.name,
                })
            }
            .await,
        )
    }

    async fn finish_server_sso(
        &self,
        request: Request<pb::FinishServerSsoRequest>,
    ) -> Result<Response<pb::FinishServerSsoResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.app.servers.get(&req.server_id).await?;
                let ran_out = || Error::FailedPrecondition("this sign-in ran out; start again".into());
                let sign_in = sso::load(&sdb.read()?, req.state.trim()).await?.ok_or_else(ran_out)?;
                if sign_in.account_id != account.id {
                    return Err(Error::denied("another account started this sign-in"));
                }
                let identity = sso::check_finish(&sign_in, &req.code, &req.secret)?;
                let now = now_ms();
                let member = sdb
                    .write(&account.id, async |conn, events| {
                        if !sso::take(conn, &sign_in.state).await? {
                            return Err(ran_out());
                        }
                        if store::load_sso(conn).await?.provider.trust_key() != sign_in.provider_key {
                            return Err(Error::FailedPrecondition(
                                "single sign-on changed while you were signing in; start again".into(),
                            ));
                        }
                        // Someone who signed in as them before and has since gone.
                        conn.execute(
                            "DELETE FROM sso_identities WHERE subject = ?1 AND user_id <> ?2
                             AND user_id NOT IN (SELECT user_id FROM members)",
                            (identity.subject.as_str(), account.id.as_str()),
                        )
                        .await?;
                        conn.execute("DELETE FROM sso_identities WHERE user_id = ?1", [account.id.as_str()]).await?;
                        let inserted = conn
                            .execute(
                                "INSERT INTO sso_identities (user_id, subject, email, name, signed_in_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                                (
                                    account.id.as_str(),
                                    identity.subject.as_str(),
                                    identity.email.as_str(),
                                    identity.name.chars().take(128).collect::<String>(),
                                    now,
                                ),
                            )
                            .await
                            .map_err(Error::from);
                        match inserted {
                            Err(err) if is_unique_violation(&err) => {
                                return Err(Error::AlreadyExists(
                                    "someone else here already signed in with that account".into(),
                                ));
                            }
                            other => other?,
                        };
                        let member = store::member(conn, &sdb.id, &account.id).await?;
                        if let Some(member) = &member {
                            events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member.clone()) }));
                        }
                        Ok(member)
                    })
                    .await?;
                tracing::info!(server = %sdb.id, account = %account.id, "signed in through a server's single sign-on");
                Ok(pb::FinishServerSsoResponse { identity: Some(identity.to_pb(now)), member })
            }
            .await,
        )
    }
}

/// Tells streams about members whose sign-in ran out between `since` and
/// `now`, so what they can see is worked out again (a MemberUpdated each).
pub async fn note_lapses(sdb: &ServerDb, since: i64, now: i64) -> Result<usize> {
    let conn = sdb.read()?;
    let sso = store::load_sso(&conn).await?;
    if !sso.required || sso.recheck_days <= 0 {
        return Ok(0);
    }
    let window = i64::from(sso.recheck_days) * 86_400_000;
    let lapsed = crate::db::query_all(
        &conn,
        "SELECT sso_identities.user_id FROM sso_identities JOIN members ON members.user_id = sso_identities.user_id
         WHERE signed_in_at + ?1 > ?2 AND signed_in_at + ?1 <= ?3",
        (window, since, now),
        |r| r.get::<String>(0),
    )
    .await?;
    if lapsed.is_empty() {
        return Ok(0);
    }
    sdb.write("", async |conn, events| {
        for user_id in &lapsed {
            if let Some(member) = store::member(conn, &sdb.id, user_id).await? {
                events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member) }));
            }
        }
        Ok(())
    })
    .await?;
    Ok(lapsed.len())
}
