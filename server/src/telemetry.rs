//! The anonymous usage signal: once a day, a handful of totals about this
//! instance go to waifu.dev so we can see how fuwa is used. No message content,
//! names, ids of people or servers, or addresses are in it. Operators turn it off
//! with FUWA_TELEMETRY=off (or DO_NOT_TRACK=1); README.md lists every field.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::app::App;
use crate::error::Result;
use crate::id::now_ms;

/// Bumped only for breaking changes; new fields can join v1.
pub const SCHEMA: &str = "fuwa.signal.v1";

const FIRST_DELAY: Duration = Duration::from_secs(5 * 60);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Serialize)]
pub struct Signal {
    pub schema: &'static str,
    /// Random, made once per instance. Lets consecutive signals be told apart
    /// from other instances' without saying anything about this one.
    pub install_id: String,
    /// Unix milliseconds.
    pub sent_at: i64,
    pub version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub uptime_seconds: u64,
    pub config: SignalConfig,
    pub totals: Totals,
}

#[derive(Debug, Serialize)]
pub struct SignalConfig {
    pub local_accounts: &'static str,
    pub linked_accounts: bool,
    pub server_creation: &'static str,
    pub encryption: bool,
    pub limits_configured: bool,
}

/// Totals at the time of sending. Lifetime counters only grow, so the difference
/// between two signals from the same instance is the activity in between.
#[derive(Debug, Default, Serialize)]
pub struct Totals {
    pub accounts: i64,
    pub accounts_active_1d: i64,
    pub accounts_active_30d: i64,
    pub servers: i64,
    pub discoverable_servers: i64,
    pub members: i64,
    pub channels: i64,
    pub messages: i64,
    pub messages_sent: i64,
    pub message_bytes: i64,
    pub attachments: i64,
    pub attachment_bytes: i64,
    pub events: i64,
    pub storage_bytes: i64,
}

pub async fn collect(app: &App) -> Result<Signal> {
    let accounts = app.node.account_counts().await?;
    let (servers, discoverable_servers) = app.servers.count();
    let mut totals = Totals {
        accounts: accounts.total,
        accounts_active_1d: accounts.active_1d,
        accounts_active_30d: accounts.active_30d,
        servers,
        discoverable_servers,
        ..Totals::default()
    };
    for id in app.servers.ids() {
        let Ok(sdb) = app.servers.get(&id).await else { continue };
        let usage = sdb.usage().await?;
        totals.members += usage.members;
        totals.channels += usage.channels;
        totals.messages += usage.messages;
        totals.messages_sent += usage.messages_sent;
        totals.message_bytes += usage.message_bytes;
        totals.attachments += usage.attachments;
        totals.attachment_bytes += usage.attachment_bytes;
        totals.events += usage.events;
        totals.storage_bytes += usage.storage_bytes;
    }
    let config = &app.config;
    Ok(Signal {
        schema: SCHEMA,
        install_id: app.node.install_id().await?,
        sent_at: now_ms(),
        version: crate::VERSION,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        uptime_seconds: app.started.elapsed().as_secs(),
        config: SignalConfig {
            local_accounts: config.local_accounts.as_str(),
            linked_accounts: false,
            server_creation: match config.server_creation {
                crate::pb::ServerCreation::Everyone => "everyone",
                crate::pb::ServerCreation::Admins => "admins",
                _ => "off",
            },
            encryption: config.encryption_key.is_some(),
            limits_configured: config.limits.any(),
        },
        totals,
    })
}

/// Sends the signal a few minutes after start and then daily, until shutdown.
pub fn spawn(app: Arc<App>) {
    if !app.config.telemetry.enabled {
        tracing::info!("anonymous usage signal is off");
        return;
    }
    tracing::info!(
        url = %app.config.telemetry.url,
        "sending an anonymous usage signal daily (counts only; set FUWA_TELEMETRY=off to stop it)"
    );
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .user_agent(format!("fuwa/{}", crate::VERSION))
            .timeout(Duration::from_secs(15))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!(error = %err, "couldn't set up the usage signal; it stays off");
                return;
            }
        };
        let mut delay = FIRST_DELAY;
        loop {
            tokio::select! {
                _ = app.shutdown.cancelled() => return,
                _ = tokio::time::sleep(delay) => {}
            }
            delay = INTERVAL;
            if let Err(err) = send(&app, &client).await {
                tracing::debug!(error = %err, "usage signal not sent; trying again tomorrow");
            }
        }
    });
}

async fn send(app: &App, client: &reqwest::Client) -> std::result::Result<(), String> {
    let signal = collect(app).await.map_err(|err| err.to_string())?;
    tracing::info!(signal = %serde_json::to_string(&signal).unwrap_or_default(), "sending usage signal");
    let response = client.post(&app.config.telemetry.url).json(&signal).send().await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("the collector answered {}", response.status()));
    }
    Ok(())
}
