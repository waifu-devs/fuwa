# Calls

Voice channels in community servers and calls in direct messages, with
sound and cameras. Screen sharing and the desktop app's calls come later;
this is how calls work now and what those build on.

## The parts

- **The media part** carries the sound and cameras. It's an SFU (a selective forwarding
  unit) built on [str0m](https://github.com/algesten/str0m) inside the `fuwa`
  binary (`server/src/rtc.rs`): each app sends its sound to it once and it
  forwards that to everyone else in the call, without decoding or mixing.
  In one process (`FUWA_ROLE=all`) it runs in that process; split, it's its
  own part (`FUWA_ROLE=media`, `cluster/media.rs`), and the directory and
  shards reach it on `FUWA_MEDIA_URL` with the cluster key. Several media
  parts share calls by rendezvous hashing over their URLs (`voice.rs`), so a
  call always lands on the same one, and adding one moves only the calls
  that land on it.
- **CallService** (`api/calls.rs`, on the shard holding the server, or the
  directory for direct messages) decides who may be where: it hands out
  places in calls, checks permissions, opens the connection on the media part
  and tells everyone who's in (`VoiceStateUpdated` and `VoiceStateRemoved` on
  the server's live stream, `CallUpdated` on the direct-message stream). Who's
  in a call is kept in memory (`voice::Voice`), never in a database: it's
  gone the moment the process is.
- **The apps** (`web/src/calls/`) join through CallService, then talk WebRTC
  with the media part.

## Joining and staying

1. The app asks `GetCallSettings` for the STUN and TURN servers, makes an
   `RTCPeerConnection` with one microphone track and a data channel named
   `fuwa`, and sends its offer with `JoinVoice` (or `JoinDmCall`).
2. CallService checks the place (CONNECT in that channel, not timed out, or
   being in the conversation), opens it on the media part and returns the
   answer and a `session_id`.
3. The media part is ICE-lite: it only offers its own addresses, and the app
   connects to it over UDP, or TCP on the same port when UDP is blocked
   (ICE-TCP).
4. As people come and go, the media part offers the app their tracks over the
   data channel (`{"type":"offer"}`); the app answers. Each track's stream id
   is the participant's account id, so an app can show, mute or turn down
   each person on their own, and put each camera in its own window.
5. The app keeps its place with `KeepVoice` (or `KeepDmCall`) every 5
   seconds, sending its own mute and deafen. A place nobody keeps for 15
   seconds is let go.

Server mute and deafen (MUTE_MEMBERS) stay with the person in the server's
file (`voice_moderation`), so leaving and joining again doesn't lift them.
Without SPEAK, people join and listen but the media part doesn't pass their
sound on. Losing CONNECT, a time-out, a kick, a ban or the channel going
away hangs them up the moment it happens (`spawn_voice_guard` watches the
server's events), and each keep checks again.

## Cameras

Every app's first offer has a place for a camera (a send-only video track)
with nothing on it. Turning the camera on puts a track there
(`replaceTrack`) and says `self_video` in the next keep; turning it off takes
the track away. Neither needs a new offer. The media part passes a camera on only while its
place says `self_video`, so an app can't film while everyone sees its camera
off. The media part only offers
someone's camera to the others once its first frame arrives, so a camera
nobody turned on costs nobody anything.

Cameras are VP8 (the media part takes only Opus and VP8, so every app can
show every other's), at up to 720p and 30 frames a second, sent in three
sizes at once (simulcast): "l" a quarter (150 kbit/s at most), "m" half
(500 kbit/s) and "h" full (1.5 Mbit/s). Each viewer gets one size of each
camera: the app says which over the data channel, by the track's mid,
`{"type":"layers","layers":{"<mid>":"h"|"m"|"l"|"off"}}`, and picks it
from how tall it shows that camera (`web/src/calls/video.ts`): up to 240
device pixels "l", up to 480 "m", bigger "h", and "off" when it isn't
showing it at all. Until it says, a viewer gets "l".

The media part sends the biggest size that isn't bigger than asked among
the sizes that came lately (a camera short on upload drops "h" first), and
switches sizes only on a keyframe, which it asks the camera's app for
(FIR), so the picture never breaks up. Each size may keep its own clock, so
the media part moves the frames' times on a switch to carry on from the
last one the viewer got.

VIDEO is a permission of its own, per channel, given wherever SPEAK was
(migration 0012). Without it, the voice state says `video_suppress`, the
media part drops the camera's frames, and the app turns the camera off.
Programs (bridges) never get cameras.

### Pop-out windows and clean feeds

Any tile pops out into a window of its own: that one person's camera, edge
to edge, at the size the window needs (full 720p for a big window), or their
avatar on their color while their camera is off. Nothing else shows unless
the mouse moves: the name, a glow while they talk, filling or fitting the
window, each remembered. The window's title is `<name> · fuwa camera` and
it's named per person, so streaming apps (OBS's Window Capture, for one)
find the same window again and take it as a clean feed of that person.
Hanging up closes them.

## Ping

The voice bar shows your ping to the media part, and clicking it opens the
last minute of it with packet loss, jitter and the route (UDP, TCP or a TURN
relay). It's read from the browser's own WebRTC stats every two seconds
(`web/src/calls/quality.ts`): the selected candidate pair's round trip,
which the browser measures with its consent checks to the media part. Only
your own; nothing about it is sent anywhere.

## Restarts

Calls ride out deploys:

- **The media part** stopping tells every app over the data channel
  (`{"type":"restarting"}`) and holds on briefly. Apps join again with the
  same `session_id` after a short random delay; their place was never let
  go, so nobody sees them leave, and sound comes back within a second or two
  of the new media part starting.
- **CallService's part** (one process, a shard, or the directory) restarting
  forgets who's in, but the media part still carries their sound. The next
  keep puts them back, only if the media part says that session is still
  connected there.
- **A connection that drops** (Wi-Fi changing, a laptop waking up) reconnects
  the same way, fetching fresh TURN credentials first.

`server/tests/calls.rs` and `server/tests/cluster.rs`
(`calls_ride_out_a_media_restart`) cover these.

## Agents, bots and apps

Programs hear voice channels and talk in them without WebRTC, through two
calls of the same API (`CallService`, gRPC):

- **ListenVoice** joins a voice channel for as long as its stream is open,
  with the account's own permissions (CONNECT to join), and everyone sees
  the program there (agents with their AGENT badge). The first message
  says it's in, with its `session_id`; then each person's sound comes as
  frames of Opus (48 kHz, 20 ms each), labelled with whose they are and
  when they were spoken. The stream keeps the place; closing it leaves.
- **SpeakVoice** sends frames of Opus to say, which go out to everyone as
  the program's own track (stream id: its account id, as for anyone),
  one every 20 ms. At most a second's worth waits at a time, so a program
  sends them about as fast as they play. It needs SPEAK, and not being
  server muted.

On the media part this is a bridge (`Sfu::bridge` in `rtc.rs`): a member of
the room with no connection, which gets the frames everyone else's apps
send, and whose queued frames are paced onto a track every app in the room
is offered. A program that can't keep up misses frames rather than holding
the call up. When the media part restarts, the shard opens the bridge again
on the one that takes over, and the program's stream carries on: nobody
sees it leave. Being taken out (a moderator, a kick, losing CONNECT, or
joining from somewhere else) ends the stream with FAILED_PRECONDITION.

The `fuwa-voice` crate (`voice/`) wraps both for Rust programs, joining
again by itself after a dropped connection:

```rust
let client = fuwa_voice::Client::connect("https://fuwa.chat", &token).await?;
let (mut heard, speaker) = client.join(&server_id, &channel_id).await?;
while let Some(frame) = heard.next().await? {
    // frame.user_id said frame.opus; decode it with any Opus library.
    speaker.say(vec![frame.opus]).await?;
}
```

`cargo run -p fuwa-voice --example parrot -- <server id> <channel id>` (with
`FUWA_URL` and an agent's `FUWA_TOKEN`) runs a bot that repeats what each
person says once they pause. Any language with gRPC can do the same with
the two calls. Calls in direct messages stay closed to programs: they're
end-to-end encrypted.

## Direct messages are end-to-end encrypted

Calls in direct messages are always end-to-end encrypted; there's no off
switch. Voice channels aren't: the media part could read their sound.

Every frame of sound is sealed in the app before it's sent, with
WebRTC's encoded transforms (`RTCRtpScriptTransform`, or Chrome's
`createEncodedStreams`), in a worker (`web/src/calls/frames.worker.ts`):

- The key comes from the conversation's MLS group (see [e2ee.md](e2ee.md)):
  `export_secret` with the label `fuwa call v1`, 32 bytes, new at every epoch.
  Every device in the conversation gets the same secret, and the server never
  does.
- Each sender seals with their own key: HKDF-SHA256 of that secret, no salt,
  info `fuwa call frame <their account id>`, as an AES-256-GCM key.
- A sealed frame is the ciphertext and tag, then a 17-byte trailer: a random
  12-byte nonce, the epoch (4 bytes, big endian) and a version byte (1). The
  epoch and version are authenticated with the frame.
- Camera frames keep their VP8 header in the clear, as other end-to-end
  encrypted calls do: the first 10 bytes of a keyframe, 3 of any other frame
  (the first bit says which). That says only whether it's a keyframe and its
  size, which the media part needs to start each viewer on a keyframe. The
  header is authenticated with the frame (it's part of the associated data,
  before the epoch and version), so changing it breaks the frame.
- A frame that can't be sealed or opened is dropped, never sent or played in
  the clear. A frame from an epoch the app doesn't have yet makes it catch up
  on the conversation and try again.

The desktop app's core (`desktop/src/core/calls.rs`) seals and opens frames
of sound the same way, checked against a frame the web app sealed; it has no
camera yet.

What the server sees of a direct-message call: that it's happening, who's in
it and since when, their mute, deafen and camera on or off, the size and
timing of the sealed frames, and which camera frames are keyframes. Not the
sound or the pictures.

## Privacy

Apps only ever connect to the media part, never to each other: there's no
peer-to-peer fallback. The media part never sends anyone another person's
addresses or candidates, and nothing about where someone connects from goes
in call events, logs or the audit log.

TURN credentials (coturn's REST scheme, from `FUWA_TURN_SECRET`) are new for
every connection, last an hour, and name no account: the username is the
expiry and a random name.

## What the media part accepts

An app's offer may send one track of sound and one of camera, receive the
others' tracks and open the data channel, and nothing else; at most 10
offers in 10 seconds. Each person's sound is capped at 80 KB a second and
1500 bytes a frame, far above any Opus voice, and so is each program's.
Each camera is capped at 1 MB a second, all its sizes together, and 512 KB
a frame. A call holds at most 99 people, programs included.

## Hosting the media part

It needs one port reachable from the internet, UDP and TCP (`FUWA_MEDIA_PORT`,
50000 by default), and to know the addresses apps reach it on
(`FUWA_MEDIA_ADDRESSES`, its own by default). See
[self-hosting.md](self-hosting.md#calls).

Railway has no public UDP. There the media part goes behind a TCP proxy
(`FUWA_MEDIA_ADDRESSES=tcp/<proxy host>:<proxy port>`), so apps use ICE-TCP,
and a TURN server helps people on networks that block unusual ports. With a
volume-less media service, deploys overlap, so the restart above is the only
interruption.

## Next

- **Screen sharing**: a second video track per person, on the same sizes
  and keyframe switching as cameras.
- **The desktop app's calls**: str0m as the WebRTC client, cpal for the
  microphone and speakers, Opus, the frame encryption it already has, and
  then cameras, with each person's camera in a native window of its own.
