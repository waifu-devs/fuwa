//! A parrot: it joins a voice channel and says back what each person said,
//! once they pause. No Opus library needed, since it repeats the frames as
//! they came (a real bot would decode them with an Opus library to hear
//! the words, and encode what it says).
//!
//!     FUWA_URL=https://fuwa.chat FUWA_TOKEN=<agent token> \
//!       cargo run -p fuwa-voice --example parrot -- <server id> <voice channel id>

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::{Instant, timeout};

/// How long someone stays quiet before the parrot repeats them.
const PAUSE: Duration = Duration::from_millis(700);
/// The most it remembers of one person at a time: 15 seconds.
const MOST: usize = 750;
/// How much bigger than someone's quiet frames a frame has to be to count
/// as talking. Apps keep sending small frames while nobody talks; this guess
/// needs no Opus decoder, but a real bot would decode and listen properly.
const LOUDER: usize = 16;

#[tokio::main]
async fn main() -> Result<(), fuwa_voice::Error> {
    let url = std::env::var("FUWA_URL").unwrap_or_else(|_| "https://fuwa.chat".into());
    let token = std::env::var("FUWA_TOKEN").expect("set FUWA_TOKEN to an agent's token");
    let mut args = std::env::args().skip(1);
    let (server, channel) = (args.next().expect("a server id"), args.next().expect("a voice channel id"));

    let client = fuwa_voice::Client::connect(&url, &token).await?;
    let (mut heard, speaker) = client.join(&server, &channel).await?;
    println!("in the channel; say something");

    // What each person said since they started, and when they last spoke.
    let mut said: HashMap<String, (Vec<Vec<u8>>, Instant)> = HashMap::new();
    let mut quiet: HashMap<String, f32> = HashMap::new();
    loop {
        if let Ok(frame) = timeout(Duration::from_millis(100), heard.next()).await {
            let Some(frame) = frame? else { break };
            // Each person's quiet: the smallest frames they send, drifting up slowly.
            let floor = quiet.entry(frame.user_id.clone()).or_insert(frame.opus.len() as f32);
            *floor = (*floor * 1.002).min(frame.opus.len() as f32);
            let talking = frame.opus.len() >= *floor as usize + LOUDER;
            if !talking && !said.contains_key(&frame.user_id) {
                continue;
            }
            let (frames, last) = said.entry(frame.user_id).or_insert_with(|| (vec![], Instant::now()));
            if frames.len() < MOST {
                frames.push(frame.opus);
            }
            if talking {
                *last = Instant::now();
            }
        }
        // Whoever paused gets repeated, one person at a time.
        let done = said.iter().find(|(_, (_, last))| last.elapsed() >= PAUSE).map(|(who, _)| who.clone());
        if let Some((frames, _)) = done.and_then(|who| said.remove(&who)) {
            println!("repeating {} ms", frames.len() * 20);
            let speaker = speaker.clone();
            tokio::spawn(async move { speaker.say(frames).await });
        }
    }
    Ok(())
}
