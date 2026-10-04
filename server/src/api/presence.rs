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
                let activities = match self.app.settings().rich_presence {
                    true => presence::check_activities(req.activities, |url| self.app.picture_link(url))?,
                    false => Vec::new(),
                };
                let settings = self.presence_settings_if_new(&caller.account.id).await?;
                self.app.presence.update(
                    &self.app.index,
                    &caller.account.id,
                    &caller.token_hash,
                    kind,
                    req.idle,
                    activities,
                    settings,
                );
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
        let account_id = caller.account.id.clone();
        let settings = self.presence_settings_if_new(&account_id).await?;
        let watch = self.app.presence.watch(&self.app.index, &account_id, settings);
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<pb::WatchPresenceResponse, Status>>(64);
        let app = self.app.clone();
        tokio::spawn(async move {
            let presence::Watch { id, snapshot, rx: mut changes } = watch;
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
                            // Fell behind: its place was given up.
                            None => {
                                let _ = tx.send(Err(Status::aborted("fell behind; watch again"))).await;
                                return;
                            }
                        },
                    }
                }
            };
            followed.await;
            app.presence.unwatch(&account_id, id);
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
                self.app.node()?.set_presence_settings(&account.id, &settings).await?;
                self.app.presence.settings_changed(&self.app.index, &account.id, settings.clone());
                Ok(pb::UpdatePresenceSettingsResponse { settings: Some(settings) })
            }
            .await,
        )
    }
}
