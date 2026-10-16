//! Streamer mode by itself, the web's "Turn on when OBS or XSplit goes
//! live", which only an app that can see which programs run can do: while
//! a streaming program is open, streamer mode is on, and it goes off again
//! when they've all closed, unless you'd turned it on yourself first. Only
//! the names of running programs are looked at, every few seconds; nothing
//! about them is kept, logged or sent.

use std::time::Duration;

/// How often the running programs are looked at.
pub const EVERY: Duration = Duration::from_secs(5);

/// Streaming programs, by file name in lowercase without ".exe": OBS on
/// every system (`obs64.exe`, `OBS` in its macOS bundle, `obs` on Linux)
/// and XSplit Broadcaster on Windows.
const PROGRAMS: &[&str] = &["obs", "obs64", "obs32", "obs-studio", "xsplit.core", "xsplitbroadcaster"];

/// Whether a program by this name is one that streams.
pub fn is_streaming_program(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    PROGRAMS.contains(&name)
}

/// Whether a streaming program is running now. Blocks while it looks.
pub fn streaming_now() -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, ProcessRefreshKind::new());
    system.processes().values().any(|p| is_streaming_program(&p.name().to_string_lossy()))
}

/// Follows streaming programs coming and going, and says when streamer mode should change.
#[derive(Default)]
pub struct Follow {
    running: bool,
    /// Streamer mode was turned on here, so it goes off with the programs.
    turned_on: bool,
}

impl Follow {
    /// Whether a streaming program runs now, and whether streamer mode is
    /// on: Some(on) when streamer mode should change.
    pub fn see(&mut self, running: bool, mode_on: bool) -> Option<bool> {
        let was = std::mem::replace(&mut self.running, running);
        match (was, running) {
            (false, true) if !mode_on => {
                self.turned_on = true;
                Some(true)
            }
            (true, false) => (std::mem::take(&mut self.turned_on) && mode_on).then_some(false),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_programs_are_known_by_name_on_every_system() {
        for name in ["obs64.exe", "OBS", "obs", "obs32.exe", "XSplit.Core.exe", "XSplitBroadcaster.exe"] {
            assert!(is_streaming_program(name), "{name}");
        }
        for name in ["obsidian", "Discord.exe", "fuwa", "", "robs"] {
            assert!(!is_streaming_program(name), "{name}");
        }
    }

    #[test]
    fn streamer_mode_follows_a_streaming_program() {
        let mut follow = Follow::default();
        assert_eq!(follow.see(false, false), None);
        // OBS opens: on; it stays on while OBS runs.
        assert_eq!(follow.see(true, false), Some(true));
        assert_eq!(follow.see(true, true), None);
        // OBS closes: off again.
        assert_eq!(follow.see(false, true), Some(false));
        assert_eq!(follow.see(false, false), None);
    }

    #[test]
    fn streamer_mode_you_turned_on_stays_on() {
        let mut follow = Follow::default();
        assert_eq!(follow.see(true, true), None);
        assert_eq!(follow.see(false, true), None);
    }

    #[test]
    fn turning_it_off_while_streaming_is_kept() {
        let mut follow = Follow::default();
        assert_eq!(follow.see(true, false), Some(true));
        // You turn it off by hand while OBS runs: it isn't turned back on.
        assert_eq!(follow.see(true, false), None);
        assert_eq!(follow.see(false, false), None);
        // Next time OBS opens, it is.
        assert_eq!(follow.see(true, false), Some(true));
    }
}
