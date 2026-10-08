//! How the call's connection to its media part is doing, the web's
//! web/src/calls/quality.ts: ping, packet loss, jitter and the route, read
//! from str0m's stats every couple of seconds, and the last minute of pings
//! for the graph.

/// Good, okay or poor, as the three signal bars show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Good,
    Okay,
    Poor,
}

/// How the sound gets to the media part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Straight there over UDP.
    Udp,
    /// UDP was blocked: ICE-TCP.
    Tcp,
}

/// Samples kept for the graph: a minute, one every two seconds.
pub const HISTORY: usize = 30;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quality {
    /// Round trip to the media part, in ms.
    pub ping: Option<u32>,
    /// Share of sound lost on the way, 0 to 1, either direction.
    pub loss: f32,
    /// In ms.
    pub jitter: u32,
    pub route: Option<Route>,
    pub level: Option<Level>,
    /// The last minute of pings, oldest first.
    pub history: Vec<u32>,
}

/// The web's `levelOf`.
pub fn level_of(ping: u32, loss: f32) -> Level {
    if ping >= 300 || loss >= 0.1 {
        Level::Poor
    } else if ping >= 120 || loss >= 0.02 {
        Level::Okay
    } else {
        Level::Good
    }
}

/// One look at the connection.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub ping: Option<u32>,
    pub loss: f32,
    pub jitter: u32,
    pub route: Option<Route>,
}

impl Quality {
    /// Takes a new sample, as the web's `watchQuality` does.
    pub fn take(&mut self, sample: Sample) {
        let ping = sample.ping.or(self.ping);
        if let Some(ping) = sample.ping {
            self.history.push(ping);
            let over = self.history.len().saturating_sub(HISTORY);
            self.history.drain(..over);
        }
        self.ping = ping;
        self.loss = sample.loss;
        self.jitter = sample.jitter;
        self.route = sample.route.or(self.route);
        self.level = ping.map(|p| level_of(p, sample.loss));
    }

    /// "42 ms", or a dash before the first sample.
    pub fn ping_text(&self) -> String {
        match self.ping {
            Some(ms) => format!("{ms} ms"),
            None => "–".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_web() {
        assert_eq!(level_of(40, 0.0), Level::Good);
        assert_eq!(level_of(120, 0.0), Level::Okay);
        assert_eq!(level_of(40, 0.02), Level::Okay);
        assert_eq!(level_of(300, 0.0), Level::Poor);
        assert_eq!(level_of(40, 0.1), Level::Poor);
    }

    #[test]
    fn keeps_a_minute_of_pings() {
        let mut q = Quality::default();
        assert_eq!(q.ping_text(), "–");
        for n in 0..40 {
            q.take(Sample { ping: Some(n), loss: 0.0, jitter: 1, route: Some(Route::Udp) });
        }
        assert_eq!(q.history.len(), HISTORY);
        assert_eq!(q.history[0], 10);
        assert_eq!(q.ping_text(), "39 ms");
        // A sample without a ping keeps the last one.
        q.take(Sample { ping: None, loss: 0.05, jitter: 2, route: None });
        assert_eq!(q.ping, Some(39));
        assert_eq!(q.level, Some(Level::Okay));
        assert_eq!(q.route, Some(Route::Udp));
    }
}
