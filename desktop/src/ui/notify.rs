//! The system's own notifications, for messages that arrive while the window
//! is in the background. Clicking one (where the system says so) opens where
//! the message is.

use futures::channel::mpsc;
use parking_lot::Mutex;

/// Where a clicked notification goes: an instance, a server (none for a
/// private conversation), and a channel or conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clicked {
    pub instance: String,
    pub server: Option<String>,
    pub channel: String,
    /// A thread reply's: the thread to open.
    pub thread: Option<String>,
}

static CLICKS: Mutex<Option<mpsc::UnboundedSender<Clicked>>> = Mutex::new(None);

/// Clicks on notifications, for the window; only the latest caller gets them.
pub fn clicks() -> mpsc::UnboundedReceiver<Clicked> {
    let (tx, rx) = mpsc::unbounded();
    *CLICKS.lock() = Some(tx);
    rx
}

/// Opens where a notification points, as clicking it would (the game
/// overlay's cards).
pub fn click(target: Clicked) {
    if let Some(tx) = CLICKS.lock().as_ref() {
        let _ = tx.unbounded_send(target);
    }
}

/// Shows a notification. It never blocks the window: the system is asked
/// from a thread of its own, which also waits for a click where it can.
pub fn show(title: String, body: String, target: Clicked) {
    std::thread::Builder::new()
        .name("fuwa-notification".into())
        .spawn(move || {
            let mut note = notify_rust::Notification::new();
            note.appname("fuwa").summary(&title).body(&body);
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                note.icon("fuwa").action("default", "Open").hint(notify_rust::Hint::Category("im.received".into()));
                match note.show() {
                    Ok(handle) => handle.wait_for_action(|action| {
                        if action == "default"
                            && let Some(tx) = CLICKS.lock().as_ref()
                        {
                            let _ = tx.unbounded_send(target.clone());
                        }
                    }),
                    Err(err) => tracing::debug!("couldn't show a notification: {err}"),
                }
            }
            #[cfg(not(all(unix, not(target_os = "macos"))))]
            {
                let _ = &target;
                if let Err(err) = note.show() {
                    tracing::debug!("couldn't show a notification: {err}");
                }
            }
        })
        .ok();
}

/// A message's text, as plain words for a notification.
pub fn plain(text: &str) -> String {
    let stripped: String = text.chars().filter(|c| !matches!(c, '*' | '_' | '~' | '`' | '>' | '#')).collect();
    let words = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.chars().count() > 160 { format!("{}…", words.chars().take(159).collect::<String>()) } else { words }
}

#[cfg(test)]
mod tests {
    #[test]
    fn notifications_read_as_plain_words() {
        assert_eq!(super::plain("**hi**\n> there `you`"), "hi there you");
        assert_eq!(super::plain(&"a".repeat(200)).chars().count(), 160);
    }
}
