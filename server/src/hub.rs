//! Fans committed events out to live subscribers, per community server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc};

use crate::pb;

/// How many events a subscriber may fall behind before it's cut off and has to
/// resubscribe from its last sequence.
const BUFFER: usize = 1024;

/// Not an event of the server's: the server moved to another shard, so
/// streams following it here end, as misrouted, and the gateway follows it
/// where it is now. Never stored, and never sent to a client.
pub fn moved_event(server_id: &str) -> pb::Event {
    pb::Event { id: String::new(), server_id: server_id.to_string(), sequence: -1, payload: None, ..Default::default() }
}

/// Whether an event is a [`moved_event`].
pub fn is_moved(event: &pb::Event) -> bool {
    event.sequence == -1 && event.payload.is_none()
}

#[derive(Default)]
pub struct Hub {
    channels: Mutex<HashMap<String, broadcast::Sender<Arc<pb::Event>>>>,
    /// Sees every event of every server, followed or not (calls use it to
    /// hang up whoever just lost their place).
    tap: Mutex<Option<mpsc::UnboundedSender<Arc<pb::Event>>>>,
    /// Sees every event too, for passing what happens in shared channels on
    /// to the servers that show them (`api::spawn_shared_fanout`).
    shared: Mutex<Option<mpsc::UnboundedSender<Arc<pb::Event>>>>,
}

impl Hub {
    pub fn subscribe(&self, server_id: &str) -> broadcast::Receiver<Arc<pb::Event>> {
        let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        channels.entry(server_id.to_string()).or_insert_with(|| broadcast::channel(BUFFER).0).subscribe()
    }

    /// Every event published from now on. One tap at a time: a new one replaces the last.
    pub fn tap(&self) -> mpsc::UnboundedReceiver<Arc<pb::Event>> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.tap.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
        rx
    }

    /// Every event published from now on, for shared channels. One at a time,
    /// like [`tap`](Self::tap).
    pub fn shared_tap(&self) -> mpsc::UnboundedReceiver<Arc<pb::Event>> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.shared.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
        rx
    }

    /// Sends events to everyone following their server. Callers publish in commit
    /// order, so subscribers see each server's events in sequence.
    pub fn publish(&self, events: impl IntoIterator<Item = pb::Event>) {
        let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        let mut tap = self.tap.lock().unwrap_or_else(|p| p.into_inner());
        let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        for event in events {
            let server_id = event.server_id.clone();
            let event = Arc::new(event);
            if tap.as_ref().is_some_and(|t| t.send(event.clone()).is_err()) {
                *tap = None;
            }
            if shared.as_ref().is_some_and(|t| t.send(event.clone()).is_err()) {
                *shared = None;
            }
            let idle = match channels.get(&server_id) {
                None => continue,
                Some(sender) if sender.receiver_count() == 0 => true,
                Some(sender) => sender.send(event).is_err(),
            };
            if idle {
                channels.remove(&server_id);
            }
        }
    }
}
