//! Fans committed events out to live subscribers, per community server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::pb;

/// How many events a subscriber may fall behind before it's cut off and has to
/// resubscribe from its last sequence.
const BUFFER: usize = 1024;

#[derive(Default)]
pub struct Hub {
    channels: Mutex<HashMap<String, broadcast::Sender<Arc<pb::Event>>>>,
}

impl Hub {
    pub fn subscribe(&self, server_id: &str) -> broadcast::Receiver<Arc<pb::Event>> {
        let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        channels.entry(server_id.to_string()).or_insert_with(|| broadcast::channel(BUFFER).0).subscribe()
    }

    /// Sends events to everyone following their server. Callers publish in commit
    /// order, so subscribers see each server's events in sequence.
    pub fn publish(&self, events: impl IntoIterator<Item = pb::Event>) {
        let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        for event in events {
            let server_id = event.server_id.clone();
            let idle = match channels.get(&server_id) {
                None => continue,
                Some(sender) if sender.receiver_count() == 0 => true,
                Some(sender) => sender.send(Arc::new(event)).is_err(),
            };
            if idle {
                channels.remove(&server_id);
            }
        }
    }
}
