# Calls

Voice channels in community servers and calls in direct messages. Video,
screen sharing and the desktop app's sound come later; this is how calls
work now and what those build on.

## The parts

- **The media part** carries the sound. It's an SFU (a selective forwarding
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
   each person on their own, and later put each camera in its own window.
5. The app keeps its place with `KeepVoice` (or `KeepDmCall`) every 5
   seconds, sending its own mute and deafen. A place nobody keeps for 15
   seconds is let go.

Server mute and deafen (MUTE_MEMBERS) stay with the person in the server's
file (`voice_moderation`), so leaving and joining again doesn't lift them.
Without SPEAK, people join and listen but the media part doesn't pass their
sound on. Losing CONNECT, a time-out, a kick, a ban or the channel going
away hangs them up the moment it happens (`spawn_voice_guard` watches the
server's events), and each keep checks again.

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
- A frame that can't be sealed or opened is dropped, never sent or played in
  the clear. A frame from an epoch the app doesn't have yet makes it catch up
  on the conversation and try again.

The desktop app's core (`desktop/src/core/calls.rs`) seals and opens frames
the same way, checked against a frame the web app sealed.

What the server sees of a direct-message call: that it's happening, who's in
it and since when, their mute and deafen, and the size and timing of the
sealed frames. Not the sound.

## Privacy

Apps only ever connect to the media part, never to each other: there's no
peer-to-peer fallback. The media part never sends anyone another person's
addresses or candidates, and nothing about where someone connects from goes
in call events, logs or the audit log.

TURN credentials (coturn's REST scheme, from `FUWA_TURN_SECRET`) are new for
every connection, last an hour, and name no account: the username is the
expiry and a random name.

## What the media part accepts

An app's offer may send one track of sound (no video yet), receive the
others' tracks and open the data channel, and nothing else; at most 10
offers in 10 seconds. Each person's sound is capped at 80 KB a second and
1500 bytes a frame, far above any Opus voice. A call holds at most 99 people.

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

- **Video and screen sharing**: more tracks per person, simulcast, and
  keyframe requests (already forwarded).
- **Pop-out windows** on the desktop app: each participant's camera in its
  own window, and clean per-person feeds for streaming (OBS). Every track is
  already its own stream, keyed by account id, so an app can take any one
  person's.
- **The desktop app's sound**: str0m as the WebRTC client, cpal for the
  microphone and speakers, Opus, and the frame encryption it already has.
