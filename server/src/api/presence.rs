//! `PresenceService`: who's online and what they're doing (docs/presence.md),
//! over `crate::presence`. Served where accounts are.

use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::error::{Error, Result};
use crate::pb::{self, presence_service_server::PresenceService};
use crate::presence::{self, RENEW};

/// How often an idle stream gets a heartbeat, so proxies don't close it.
const HEARTBEAT: Duration = Duration::from_secs(25);
/// What an open stream gets when the instance stops.
const RESTARTING: &str = "this instance is restarting; watch again";

type WatchStream = Pin<Box<dyn Stream<Item = Result<pb::WatchPresenceResponse, Status>> + Send>>;

impl Api {
    /// Someone's saved settings, if they aren't known to `presence` yet.
    async fn presence_settings_if_new(&self, account_id: &str) -> Result<Option<pb::PresenceSettings>> {
        if self.app.presence.knows(account_id) {
            return Ok(None);
        }
        Ok(Some(self.app.node()?.presence_settings(account_id).await?))
    }
}

/// A watch stream's place in `presence`, given back when dropped.
struct Watching {
    app: std::sync::Arc<crate::app::App>,
    account_id: String,
    id: u64,
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.app.presence.unwatch(&self.account_id, self.id);
    }
}

#[tonic::async_trait]
impl PresenceService for Api {
    async fn update_presence(
        &self,
        request: Request<pb::UpdatePresenceRequest>,
    ) -> Result<Response<pb::UpdatePresenceResponse>, Status> {
        respond(
            async {
                let caller = self.caller(request.metadata()).await?;
                let req = request.into_inner();
                let kind = match req.app.as_str() {
                    "web" | "desktop" | "agent" => req.app.as_str(),
                    _ => "other",
                };
                let account_id = &caller.account.id;
                let activities = match self.app.settings().rich_presence {
                    true => presence::check_activities(req.activities, |url| {
                        match self.app.presence.allow_picture(account_id, url) {
                            true => self.app.picture_link(url),
                            false => String::new(),
                        }
                    })?,
                    false => Vec::new(),
                };
                let presence = &self.app.presence;
                let update = |activities, settings| {
                    let (index, session) = (&self.app.index, &caller.token_hash);
                    presence.update(index, account_id, session, kind, req.idle, activities, settings)
                };
                let settings = self.presence_settings_if_new(account_id).await?;
                // Forgotten between the check and the update: load them for real.
                if !update(activities.clone(), settings) {
                    let saved = self.app.node()?.presence_settings(account_id).await?;
                    update(activities, Some(saved));
                }
                Ok(pb::UpdatePresenceResponse { renew_seconds: RENEW.as_secs() as u32 })
            }
            .await,
        )
    }

    type WatchPresenceStream = WatchStream;

    async fn watch_presence(
        &self,
        request: Request<pb::WatchPresenceRequest>,
    ) -> Result<Response<WatchStream>, Status> {
        let caller = self.caller(request.metadata()).await?;
        // Agents report their own presence but don't watch anyone's.
        if caller.account.kind == pb::AccountKind::Agent {
            return Err(Error::PermissionDenied("agents can't watch presence".into()).into());
        }
        let account_id = caller.account.id.clone();
        let settings = self.presence_settings_if_new(&account_id).await?;
        let watch = self.app.presence.watch(&self.app.index, &account_id, settings);
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<pb::WatchPresenceResponse, Status>>(64);
        let app = self.app.clone();
        tokio::spawn(async move {
            let presence::Watch { id, snapshot, rx: mut changes } = watch;
            // Gives the stream's place back however this task ends.
            let _watching = Watching { app: app.clone(), account_id, id };
            let followed = async {
                let send = async |item| tx.send(item).await.is_ok();
                for presence in snapshot {
                    if !send(Ok(pb::WatchPresenceResponse { presence: Some(presence), ready: false })).await {
                        return;
                    }
                }
                if !send(Ok(pb::WatchPresenceResponse { presence: None, ready: true })).await {
                    return;
                }
                let mut heartbeat = tokio::time::interval(HEARTBEAT);
                heartbeat.tick().await;
                loop {
                    tokio::select! {
                        _ = app.shutdown.cancelled() => {
                            let _ = tx.try_send(Err(Status::unavailable(RESTARTING)));
                            return;
                        }
                        _ = tx.closed() => return,
                        _ = heartbeat.tick() => {
                            // A session signed out elsewhere stops hearing.
                            let live = async { app.node()?.session_live(&caller.token_hash).await }.await;
                            if matches!(live, Ok(false)) {
                                let _ = tx.send(Err(Error::Unauthenticated.into())).await;
                                return;
                            }
                            if !send(Ok(pb::WatchPresenceResponse::default())).await {
                                return;
                            }
                        }
                        change = changes.recv() => match change {
                            Some(presence) => {
                                if !send(Ok(pb::WatchPresenceResponse { presence: Some(presence), ready: false })).await {
                                    return;
                                }
                            }
                            // Fell behind, or a newer stream took its place.
                            None => {
                                let _ = tx.send(Err(Status::aborted("this stream ended; watch again"))).await;
                                return;
                            }
                        },
                    }
                }
            };
            followed.await;
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn get_presence_settings(
        &self,
        request: Request<pb::GetPresenceSettingsRequest>,
    ) -> Result<Response<pb::GetPresenceSettingsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let settings = self.app.node()?.presence_settings(&account.id).await?;
                Ok(pb::GetPresenceSettingsResponse { settings: Some(settings) })
            }
            .await,
        )
    }

    async fn update_presence_settings(
        &self,
        request: Request<pb::UpdatePresenceSettingsRequest>,
    ) -> Result<Response<pb::UpdatePresenceSettingsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let settings = request.into_inner().settings.ok_or_else(|| Error::invalid("settings are required"))?;
                let settings = presence::check_settings(settings)?;
                let node = self.app.node()?;
                let before = node.presence_settings(&account.id).await?;
                node.set_presence_settings(&account.id, &settings).await?;
                self.app.presence.settings_changed(&self.app.index, &account.id, settings.clone());
                // Friends see invisible people offline too.
                let invisible = pb::PresenceStatus::Invisible as i32;
                if (before.status == invisible) != (settings.status == invisible)
                    && self.app.friends().is_ok_and(|friends| friends.is_online(&account.id))
                {
                    super::friends::announce(self.app.clone(), account.id.clone());
                }
                Ok(pb::UpdatePresenceSettingsResponse { settings: Some(settings) })
            }
            .await,
        )
    }
}
