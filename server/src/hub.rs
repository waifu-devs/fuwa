//! Fans committed events out to live subscribers, per community server.

use std::collections::{HashMap, HashSet};
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
    /// Sees every event too, so the search indexer knows which servers have
    /// new messages to index (`search::spawn`).
    search: Mutex<Option<mpsc::UnboundedSender<Arc<pb::Event>>>>,
    /// Sees every event too, so agents' endpoints hear of new events in the
    /// servers they're in (`endpoints::spawn_deliveries`). Bounded: deliveries
    /// read the server's log themselves, so what doesn't fit is only a word.
    agents: Mutex<Option<mpsc::Sender<Arc<pb::Event>>>>,
    /// Servers with events the agents' tap had no room for, to wake later.
    agents_missed: Mutex<HashSet<String>>,
}

impl Hub {
    pub fn subscribe(&self, server_id: &str) -> broadcast::Receiver<Arc<pb::Event>> {
        let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        channels.entry(server_id.to_string()).or_insert_with(|| broadcast::channel(BUFFER).0).subscribe()
    }

    /// Streams following a server on this part right now.
    pub fn followers(&self, server_id: &str) -> usize {
        let channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        channels.get(server_id).map_or(0, broadcast::Sender::receiver_count)
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

    /// Every event published from now on, for the search indexer. One at a
    /// time, like [`tap`](Self::tap).
    pub fn search_tap(&self) -> mpsc::UnboundedReceiver<Arc<pb::Event>> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.search.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
        rx
    }

    /// Events published from now on, for agents' endpoints, as many as fit;
    /// servers whose events didn't are in [`agents_missed`](Self::agents_missed).
    /// One at a time, like [`tap`](Self::tap).
    pub fn agents_tap(&self) -> mpsc::Receiver<Arc<pb::Event>> {
        let (tx, rx) = mpsc::channel(BUFFER);
        *self.agents.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
        rx
    }

    /// The servers with events the agents' tap had no room for since last asked.
    pub fn agents_missed(&self) -> Vec<String> {
        self.agents_missed.lock().unwrap_or_else(|p| p.into_inner()).drain().collect()
    }

    /// Sends events to everyone following their server. Callers publish in commit
    /// order, so subscribers see each server's events in sequence.
    pub fn publish(&self, events: impl IntoIterator<Item = pb::Event>) {
        let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        let mut tap = self.tap.lock().unwrap_or_else(|p| p.into_inner());
        let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        let mut search = self.search.lock().unwrap_or_else(|p| p.into_inner());
        let mut agents = self.agents.lock().unwrap_or_else(|p| p.into_inner());
        for event in events {
            let server_id = event.server_id.clone();
            let event = Arc::new(event);
            if tap.as_ref().is_some_and(|t| t.send(event.clone()).is_err()) {
                *tap = None;
            }
            if shared.as_ref().is_some_and(|t| t.send(event.clone()).is_err()) {
                *shared = None;
            }
            if search.as_ref().is_some_and(|t| t.send(event.clone()).is_err()) {
                *search = None;
            }
            if let Some(t) = agents.as_ref() {
                match t.try_send(event.clone()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        self.agents_missed.lock().unwrap_or_else(|p| p.into_inner()).insert(server_id.clone());
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => *agents = None,
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn event(server_id: &str, sequence: i64) -> pb::Event {
        pb::Event { server_id: server_id.into(), sequence, ..Default::default() }
    }

    #[test]
    fn agents_tap_holds_so_much_and_names_the_servers_past_it() {
        let hub = Hub::default();
        let mut tap = hub.agents_tap();
        hub.publish((1..=BUFFER as i64).map(|n| event("a", n)));
        hub.publish([event("b", 1), event("c", 1), event("b", 2)]);
        let mut held = 0;
        while tap.try_recv().is_ok() {
            held += 1;
        }
        assert_eq!(held, BUFFER);
        let mut missed = hub.agents_missed();
        missed.sort();
        assert_eq!(missed, ["b", "c"]);
        assert!(hub.agents_missed().is_empty(), "asking takes them");
        hub.publish([event("d", 1)]);
        assert_eq!(tap.try_recv().unwrap().server_id, "d", "with room again, events come through");
    }
}
