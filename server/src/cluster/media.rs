//! A media part (`FUWA_ROLE=media`): carries calls' sound for a split
//! instance. It keeps nothing and knows nobody: the shards and the directory,
//! which keep who's in each call, open and close connections on it over
//! `MediaService` with the cluster key. Apps reach it directly on
//! FUWA_MEDIA_PORT (UDP, or TCP), at the addresses it hands out in its
//! answers (FUWA_MEDIA_ADDRESSES).
//!
//! When it stops (a deploy), it tells every app in a call to join again
//! ("restarting" on their data channel), and they do at once, through the
//! part that keeps their place, onto the media part that's up then. Nobody
//! leaves a call for it; the sound drops for about as long as that takes.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use axum::routing::get;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

use crate::config::Config;
use crate::cpb;
use crate::rtc::{Bridged, May, Sfu};

struct Internal(Sfu);

type BridgeStream = Pin<Box<dyn futures::Stream<Item = Result<cpb::BridgeResponse, Status>> + Send>>;

#[tonic::async_trait]
impl cpb::media_service_server::MediaService for Internal {
    type BridgeStream = BridgeStream;

    async fn bridge(&self, request: Request<cpb::BridgeRequest>) -> Result<Response<BridgeStream>, Status> {
        let r = request.into_inner();
        let heard = self
            .0
            .bridge(
                &r.room,
                &r.participant,
                &r.session_id,
                May { speak: r.may_speak, hear: r.may_hear, video: false, screen: false },
            )
            .await?;
        let events = tokio_stream::wrappers::ReceiverStream::new(heard).map(|bridged| {
            let event = match bridged {
                Bridged::Frame(h) => cpb::bridge_response::Event::Frame(cpb::HeardFrame {
                    participant: h.participant,
                    frame: h.frame,
                    timestamp: h.timestamp,
                }),
                Bridged::Ended(ending) => cpb::bridge_response::Event::Ended(ending.as_str().into()),
            };
            Ok(cpb::BridgeResponse { event: Some(event) })
        });
        Ok(Response::new(Box::pin(events)))
    }

    async fn speak(&self, request: Request<cpb::SpeakRequest>) -> Result<Response<cpb::SpeakResponse>, Status> {
        let r = request.into_inner();
        let queued = self.0.speak(&r.room, &r.participant, &r.session_id, r.frames, r.interrupt).await?;
        Ok(Response::new(cpb::SpeakResponse { queued: queued as u32 }))
    }

    async fn open(&self, request: Request<cpb::OpenRequest>) -> Result<Response<cpb::OpenResponse>, Status> {
        let r = request.into_inner();
        let answer = self
            .0
            .open(
                &r.room,
                &r.participant,
                &r.session_id,
                &r.offer,
                May { speak: r.may_speak, hear: r.may_hear, video: r.may_video, screen: r.may_screen },
            )
            .await?;
        Ok(Response::new(cpb::OpenResponse { answer }))
    }

    async fn close(&self, request: Request<cpb::CloseRequest>) -> Result<Response<cpb::CloseResponse>, Status> {
        let r = request.into_inner();
        let participant = Some(r.participant.as_str()).filter(|p| !p.is_empty());
        self.0.close(&r.room, participant, Some(r.session_id.as_str())).await?;
        Ok(Response::new(cpb::CloseResponse {}))
    }

    async fn update(&self, request: Request<cpb::UpdateRequest>) -> Result<Response<cpb::UpdateResponse>, Status> {
        let r = request.into_inner();
        let session = (!r.session_id.is_empty()).then_some(r.session_id.as_str());
        let connected = self
            .0
            .update(
                &r.room,
                &r.participant,
                session,
                May { speak: r.may_speak, hear: r.may_hear, video: r.may_video, screen: r.may_screen },
            )
            .await?;
        Ok(Response::new(cpb::UpdateResponse { connected }))
    }
}

/// A media part's internal API, carrying calls until `calls_stop`.
pub async fn start(config: &Config, calls_stop: CancellationToken) -> Result<(axum::Router, Sfu), String> {
    let media = config.media.clone().ok_or("a media part needs FUWA_MEDIA_PORT")?;
    let sfu = Sfu::start(media, calls_stop).await.map_err(|err| err.to_string())?;
    let (_, health) = tonic_health::server::health_reporter();
    let key: Arc<str> = config.cluster.key.as_deref().unwrap_or_default().into();
    let router = tonic::service::Routes::new(cpb::media_service_server::MediaServiceServer::new(Internal(sfu.clone())))
        .add_service(health)
        .into_axum_router()
        .route("/healthz", get(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(key, super::require_key));
    Ok((router, sfu))
}

/// Serves a media part until Ctrl-C or SIGTERM.
pub async fn run(config: Config, address: SocketAddr) -> Result<(), String> {
    let listener =
        tokio::net::TcpListener::bind(address).await.map_err(|err| format!("couldn't listen on {address}: {err}"))?;
    let shutdown = CancellationToken::new();
    // Calls stop a moment after the API does, so their "restarting" goes out.
    let calls_stop = CancellationToken::new();
    let (router, sfu) = start(&config, calls_stop.clone()).await?;
    tracing::info!(
        version = crate::VERSION,
        role = "media",
        %address,
        port = sfu.config().port,
        addresses = ?sfu.describe(),
        "fuwa is up"
    );
    crate::app::spawn_signal_handler(shutdown.clone());
    let stopping = shutdown.clone();
    let served = axum::serve(crate::app::no_delay(listener), router)
        .with_graceful_shutdown(async move { stopping.cancelled().await })
        .into_future();
    let calls = async {
        shutdown.cancelled().await;
        calls_stop.cancel();
        // Long enough for every app to hear it.
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    };
    let (served, ()) = tokio::join!(served, calls);
    served.map_err(|err| format!("server error: {err}"))
}
