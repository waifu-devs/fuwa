//! App settings on every device (docs/app-settings.md): reading the
//! instance's document, merging it with this computer's settings and writing
//! it back, the web's `fuwa/app-settings.ts`. What travels and how it's
//! merged is `settings_sync.rs`; this is when and where.
//!
//! An instance with the `app-settings-sync` feature is read after its first
//! snapshot and whenever the window comes back to the front (at most once a
//! minute), and written a couple of seconds after the last change here. The
//! instance's version and document are kept per instance and account, so a
//! change goes out without reading first; a write refused because another
//! device wrote meanwhile reads again and retries. Turning `sync_settings`
//! off stops all of it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::Mutex;

use crate::core::settings_sync::{self, Doc, FEATURE, PUSH_AFTER, REFRESH_EVERY, TRIES};
use crate::core::{Area, Core, store};
use crate::pb;
use crate::rpc;

/// What the core keeps for syncing settings.
#[derive(Default)]
pub struct SyncState {
    /// The instance's version and document as last read or written, by `instance/account`.
    remote: Mutex<HashMap<String, (u64, Doc)>>,
    /// Bumped by each change here; a push waits until it stops moving.
    changes: AtomicU64,
    /// When coming back to the window last read everything again.
    refreshed: Mutex<Option<Instant>>,
    /// Bumped whenever settings from another device were put in, so the window takes them.
    taken: AtomicU64,
    /// One sync at a time, so two don't apply the same settings over each other.
    turn: tokio::sync::Mutex<()>,
}

impl Core {
    /// Reads the instance's settings, takes what's newer there and sends
    /// what's newer here. With `read_first` off, starts from the document as
    /// last seen (for sending a change).
    pub(crate) async fn sync_app_settings(&self, key: &str, read_first: bool) {
        let _turn = self.settings.turn.lock().await;
        if !self.prefs_now().sync_settings {
            return;
        }
        let Some(api) = self.api(key) else { return };
        if api.token().is_none() {
            return;
        }
        let me = self
            .shared
            .read(|s| s.instance(key).filter(|i| i.has(FEATURE)).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let Some(me) = me else { return };
        let place = format!("{key}/{me}");
        let mut known = if read_first { None } else { self.settings.remote.lock().get(&place).cloned() };
        for _ in 0..TRIES {
            let (version, remote) = match known.take() {
                Some(known) => known,
                None => match rpc!(api.account(), get_app_settings(pb::GetAppSettingsRequest {})).await {
                    // Another format is a newer app's: leave it be.
                    Ok(res) => match settings_sync::read(&res.document) {
                        Some(doc) => (res.version, doc),
                        None => return,
                    },
                    Err(_) => return,
                },
            };
            self.settings.remote.lock().insert(place.clone(), (version, remote.clone()));
            let plan = settings_sync::plan(&self.prefs_now(), &remote);
            if !plan.take.is_empty() {
                self.settings.taken.fetch_add(1, Ordering::SeqCst);
                self.change_prefs(
                    |p| {
                        if !settings_sync::take(p, &plan.take).is_empty() {
                            p.tidy();
                        }
                    },
                    false,
                );
            }
            if !plan.push {
                return;
            }
            let request = pb::SetAppSettingsRequest { document: settings_sync::write(&plan.merged), version };
            match rpc!(api.account(), set_app_settings(request)).await {
                Ok(res) => {
                    self.settings.remote.lock().insert(place, (res.version, plan.merged));
                    return;
                }
                // Another device wrote meanwhile: read it and merge again.
                Err(err) if err.code == tonic::Code::FailedPrecondition => {}
                Err(_) => return,
            }
        }
    }

    /// Goes up each time settings from another device were put in.
    pub fn settings_taken(&self) -> u64 {
        self.settings.taken.load(Ordering::SeqCst)
    }

    /// Every instance followed, read and merged at once (sync turned back on).
    pub(crate) fn sync_app_settings_everywhere(&self, read_first: bool) {
        let Some(core) = self.this() else { return };
        let keys: Vec<String> = self.engines.lock().keys().cloned().collect();
        self.runtime.spawn(async move {
            for key in keys {
                core.sync_app_settings(&key, read_first).await;
            }
        });
    }

    /// A synced setting changed here: it goes out once changes stop for a moment.
    pub(crate) fn app_settings_changed(&self) {
        let n = self.settings.changes.fetch_add(1, Ordering::SeqCst) + 1;
        let weak = self.this.get().cloned();
        self.runtime.spawn(async move {
            tokio::time::sleep(PUSH_AFTER).await;
            let Some(core) = weak.and_then(|w| w.upgrade()) else { return };
            if core.settings.changes.load(Ordering::SeqCst) == n {
                core.sync_app_settings_everywhere(false);
            }
        });
    }

    /// The window came back to the front: reads what other devices may have
    /// changed without an event (settings, your own profile), at most once a minute.
    pub fn window_focused(&self) {
        {
            let mut last = self.settings.refreshed.lock();
            if last.is_some_and(|at| at.elapsed() < REFRESH_EVERY) {
                return;
            }
            *last = Some(Instant::now());
        }
        let Some(core) = self.this() else { return };
        let keys: Vec<String> = self.engines.lock().keys().cloned().collect();
        self.runtime.spawn(async move {
            for key in keys {
                core.refresh_me(&key).await;
                core.sync_app_settings(&key, true).await;
            }
        });
    }

    /// Reads you and your profile again: a decoration or effect changed on
    /// another device reaches this one only through a server you share with
    /// someone, and an effect alone through nothing. The store changes only
    /// when they did.
    pub(crate) async fn refresh_me(&self, key: &str) {
        let Some(api) = self.api(key) else { return };
        if api.token().is_none() {
            return;
        }
        let Ok(me) = rpc!(api.auth(), get_me(pb::GetMeRequest {})).await else { return };
        let Some(user) = me.user else { return };
        self.shared.instance_if(key, &[Area::All], |i| {
            let changed = i.me.as_ref() != Some(&user) || i.admin != me.admin;
            if changed {
                store::update_user(i, &user);
                i.me = Some(user.clone());
                i.admin = me.admin;
            }
            changed
        });
        let Ok(res) = rpc!(api.auth(), get_profile(pb::GetProfileRequest { user_id: user.id.clone() })).await else {
            return;
        };
        if let Some(fresh) = res.profile.and_then(|p| p.user) {
            self.shared.instance_if(key, &[Area::All], |i| {
                let changed = i.users.get(&fresh.id) != Some(&fresh);
                if changed {
                    store::update_user(i, &fresh);
                }
                changed
            });
        }
    }
}

/// The core as an `Arc`, for tasks that outlive the call that starts them.
impl Core {
    fn this(&self) -> Option<Arc<Core>> {
        self.this.get().and_then(|w| w.upgrade())
    }
}
