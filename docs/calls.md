# Calls

Voice channels in community servers and calls in direct messages, with
sound, cameras and shared screens, in the web app and the desktop app alike.

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
So does a moderator turning someone's camera and shared screen off
(`ModerateVoice` with `server_video_off`, also MUTE_MEMBERS, only on people
ranked below them): the media part stops passing both (and the screen's
sound) at once, the person stays in the call with their voice, their app
turns the camera and share off and says a moderator did it, and they can't
turn either on in that server until a moderator lifts it. Everyone sees it
as `VoiceState.server_video_off`.
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
show every other's), as sharp and smooth as the camera gives, up to 1080p
and 60 frames a second, sent in three sizes at once (simulcast): "l" a
quarter at up to 15 frames a second, "m" half at up to 30 and "h" all of it.
Each size's bitrate follows the pixels it has and its frame rate,
`max(150 kbit/s, pixels × 1.65 × (fps / 30)^0.6)` bits a second: 720p30 is
about 1.5 Mbit/s, 1080p60 about 5.2, and every size of a 1080p60 camera
together about 6.3, inside the 2 MiB a second the media part lets one
camera send. Apps size the encodings from what the camera actually gives,
so one that only does 720p30 sends that.

Three ceilings can hold cameras lower, and apps send the lowest of them:
the person's own (Resolution and Frame rate under Camera quality in Voice &
video settings, "Best" unless changed), the instance's
(`InstanceSettings.camera_max_height` and `camera_max_fps`,
`FUWA_CAMERA_MAX_HEIGHT` and `FUWA_CAMERA_MAX_FPS`, on the Calls page; apps
read it from `GetCallSettings`) and, in a server's voice channels, the
server's (`Server.camera_max_height` and `camera_max_fps`, set by someone
with MANAGE_SERVER under Calls & recordings, audited). None is set unless
someone sets it. A ceiling changing while a camera is on opens it again at
the new one. The math is `web/src/lib/camera-quality.ts` and the desktop's
`core/voice/ceiling.rs`, number for number. The ceilings are what apps keep
to, like the encodings themselves: the media part enforces only its byte
budget per camera.

Each viewer gets one size of each
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

## Shared screens

A shared screen works as a second camera. Every app's first offer has a
second send-only video track after the camera's, empty until someone shares
(`getDisplayMedia`), and the media part takes an app's first video track as
its camera and the second as its screen. Sharing says `self_stream` in the
next keep, and the media part passes the screen on only while it does.

The others get it on a stream of its own, named for its person and
`-screen` (`<account id>-screen`), which is how apps tell it from the
camera. A screen goes out in the same three sizes as a camera with more
bits, since text needs them, and switches sizes the same way. The person
sharing picks how sharp and how smooth: the full size at up to 720p, 1080p
(the default) or 1440p, at 15, 30 (the default) or 60 frames a second. The
quarter size stays at 5 frames a second and 200 kbit/s and the half at 15
and 700 kbit/s (1 Mbit/s at 1440p); the full size gets from 1.2 Mbit/s
(720p at 15) to 6 Mbit/s (1440p at 60), the table in the web's
`lib/screen-share.ts` and the desktop's `vp8::screen_sizes`, so either app
sends the same. Below 60 a share is marked as detail, so browsers keep text
sharp and drop frames first; at 60 as motion, for games and video.

Both apps ask before sharing, in a dialog: what to share (on the desktop,
every screen and window as a small picture of it, each taken from one
frame; on the web, whether the browser's own picker opens on screens,
windows or tabs, where Chrome and Edge let it), the size and frame rate,
and the sound. The choices are kept for next time and for the shortcut. VIDEO covers
screens too, and in direct messages a screen is end-to-end encrypted like
a camera.

### A screen's sound

A screen can bring its sound: a tab's, or the whole system's where the
browser and the system allow it. Where the instance offers it
(`GetCallSettingsResponse.screen_sound`), every app's first offer has a
second send-only audio track after the microphone's, empty until a share
brings sound, and the media part takes an app's second audio track as its
screen's sound. It passes that on exactly when it passes the screen on
(sharing, with VIDEO), on the screen's stream (`<account id>-screen`), so
apps play it beside the sharer's voice, never mixed into it, and it shares
the voice's caps (80 KB a second, 1500 bytes a frame). In direct messages it
is end-to-end encrypted like the voice. Programs and server recordings get
voices only, never a screen's sound (a recording with video keeps the
screen's picture, not its sound).

The share dialog asks whether the sound goes too. The sound goes as it is, no
echo cancelling or noise suppression, which spoil music, at up to 128 kbit/s,
and the app's own sound is left out, so nobody hears the call back: the
browser is asked to leave the page's out, and the desktop app shares
everything the computer plays but itself (WASAPI's process loopback on
Windows 10 version 2004 and later, ScreenCaptureKit on macOS 13 and later,
and on Linux each app's stream but its own through PulseAudio or PipeWire,
mixed in the app; `desktop/src/core/voice/screen_sound/`). On the desktop
the sound is the whole computer's whatever is shared, a window too. While sharing, the sound button on your screen turns its sound
off for everyone and back on, without stopping the share; on someone else's
screen it turns it off for you. Browsers differ, and the app says which case
you hit rather than sharing in silence: Firefox and Safari share pictures
only; Chrome and Edge share a tab's sound everywhere, the whole screen's only
where the system lets them (Windows and ChromeOS), and never a single
window's.

Apps show shared screens above everyone's tiles, whole (never cropped),
with a LIVE mark, and pop them out like cameras: the window is titled
`<name>'s screen · fuwa`.

## Recordings

Anyone may record a call's sound on their own device: the app mixes
everyone's sound as they hear it (at the volumes they gave people) with
their own microphone as it goes out, records it with `MediaRecorder` (Opus,
128 kbit/s) and saves the file when they stop or hang up. What they hear
includes the sound of screens others share, so that's in it too. Nothing is sent
anywhere. Recording says `self_record` in the next keep, so everyone sees
it: a mark by their name, a "Recording" pill on the channel, and a beep and
a note when someone starts. That's a courtesy, not a lock: anyone can record
what their own speakers play.

In voice channels it needs RECORD, a permission nobody has by default
(admins grant it per role or channel); without it the voice state says
`record_suppress` and the instance clears `self_record` (and
`server_record`). In direct messages either person may. Servers made
before RECORD existed have it on no role, Admin included, so their admins
grant it themselves; new servers' Admin role starts with it.

### On the server

Voice channels can also be recorded on the server: everyone's sound, a
track per person. Someone with RECORD says `server_record` in their keep
(JoinVoice, KeepVoice); everyone sees it in the voice state ("Recording on
the server" on the channel, a mark by their name, a beep and a note). The
recording runs while anyone in the channel has it on, and ends when the last
of them turns it off or leaves. Admins turn it off for the whole instance
with `FUWA_CALL_RECORDINGS=off` (or the Calls page); `GetCallSettings` says
whether it's on (`recordings`). It's on by default because RECORD is on no
role until a server's admins grant it (only new servers' Admin role starts
with it).

Neither size nor age is capped by default. Instance admins can cap how much
each server's recordings take, all channels together
(`ServerLimits.recording_bytes`: `FUWA_LIMIT_RECORDING_STORAGE` for every
server, or one server's own cap). At the cap the recording going on stops,
no new one starts, and whoever asked hears why (`recordings_full` on
JoinVoice and KeepVoice) until some are deleted; `ListRecordings` says what
they take and the cap. And finished recordings can delete themselves after a
number of days (`FUWA_CALL_RECORDINGS_KEEP_DAYS`, or the Calls page),
files and replica copies included; the part holding each server looks for
them hourly.

The part keeping the channel's places listens through a bridge on the media
part, like a program with ListenVoice, as nobody anyone sees, and writes
each person's frames as they came (never decoded) to their own Ogg Opus
file, with silence in between where their clock says, so every track starts
when the recording did, ends when it did, and lines up with the others in
an editor. A part that stops (a deploy) finishes its files; one that
crashed has lost at most the last second, and the recording ends where its
files do. After a restart, the apps' next keep starts a new recording.

Files are under `<data>/recordings/<server>/<recording>/`, and the server's
file lists them. With `FUWA_ENCRYPTION_KEY` they're sealed: each Ogg page
is ChaCha20-Poly1305'd under a key derived (HKDF) from the instance's key
for that file alone, and unsealed as it's downloaded. A split instance's
replica copies finished ones to its bucket (`recordings/<server>/…`), where
downloads come from when a shard doesn't have the files. People with RECORD
in the channel list them (`ListRecordings`, the one going on included),
download a person's track (`DownloadRecording`, plain Ogg Opus); whoever
started one, or someone with MANAGE_CHANNELS there, deletes it
(`DeleteRecording`). Apps also save all of a recording's tracks as one
.zip. Recordings stay when their server is deleted, alongside its file in
`deleted/`.

Direct-message calls are never recorded on the server: their sound is
end-to-end encrypted, so all it could keep is ciphertext.

#### With video

Where the instance allows it (`FUWA_CALL_RECORDING_VIDEO`, or Video in
recordings on the Calls page; off unless turned on), a server can keep
cameras and shared screens too (`Server.record_video`, under Recordings in
its settings, for someone with MANAGE_SERVER). It's off until someone turns
it on. Changing it ends the recording going on, and everyone's Record on
the server with it: KeepVoice says why (`recording_ended`), and nothing
starts again until someone presses Record, so a new recording starts the
way any does, for everyone to see and hear, and is all one or the other
(`Recording.video`). Everyone in the call sees "Recording with video" while
it is. The instance turning it off turns it off in every server and ends the
recordings filming. Recordings on someone's own device stay sound only.

The bridge then watches as well as listens: the media part passes it each
camera's and screen's VP8 frames as they came, starting on a keyframe (it
asks the sender for one when the bridge first sees a picture, and again if
the bridge fell behind and left frames out). Cameras come at half their full
size (simulcast "m", or the nearest one sent); screens as they're sent. Each
person's camera and screen go to their own WebM file next to their sound
(`<account>.camera.webm`, `<account>.screen.webm`): the frames untouched,
never decoded or put together into one picture, which would take the CPU
to decode and encode every frame. A picture starts at its first keyframe,
timed from the recording's start on the camera's own clock, so it lines up
with everyone's sound in an editor; one that goes off and on again picks up
where it came back. Files are written a cluster (about two seconds) at a
time, with the segment's size left unknown as a live stream's is, so a
crash loses at most the last two seconds. Sealed files have their own keys
(the person's sound, camera and screen each one).

`RecordingTrack` says how big each person's camera and screen files are
(zero for none), and `DownloadRecording` takes `part` to get them. Pictures
count against the same storage cap, and delete with the recording. An hour
of one person's camera at half size is around 100 MB, against 15 to 30 MB
for their sound: an instance letting servers record video wants a storage
cap (`FUWA_LIMIT_RECORDING_STORAGE`) and room for it. Pictures may take at
most half of a bridge's queue, so sound always gets through.

### Pop-out windows and clean feeds

Any tile pops out into a window of its own: that one person's camera, edge
to edge, at the size the window needs (all of it for a big window), or their
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
  While nobody says anything a keepalive comes every 15 seconds, so a
  program can tell a quiet call from a stream that's gone.
- **SpeakVoice** sends frames of Opus to say, which go out to everyone as
  the program's own track (stream id: its account id, as for anyone),
  one every 20 ms. At most a second's worth waits at a time, so a program
  sends them about as fast as they play. `interrupt` drops whatever is
  still waiting first, so a program that's talked over stops at once (with
  no frames, it just stops). It needs SPEAK, and not being server muted.

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
of sound and camera the same way, checked against frames the web app's
worker code sealed.

What the server sees of a direct-message call: that it's happening, who's in
it and since when, their mute, deafen, camera and shared screen on or off, the size and
timing of the sealed frames, and which camera frames are keyframes. Not the
sound or the pictures.

## The desktop app

Voice channels and direct-message calls, with cameras and shared screens
(`desktop/src/core/voice`). It's the web app's call in Rust: str0m as the WebRTC client (the library the media part runs),
Opus at 48 kHz in 20 ms frames, and cpal for the system's default
microphone and speakers at whatever rate they run, resampled to and from
48 kHz on a thread of their own. It joins, keeps its place every 5 s (and at
once when you mute or deafen), answers the media part's offers on the
"fuwa" data channel, and rejoins with the same session after a drop or a
"restarting". Deafening also mutes, as on the web.

It connects only to the addresses in the media part's answer: over UDP,
and at the same time over ICE-TCP (RFC 4571 framing) to its TCP addresses,
which is how it gets through on hosts that only proxy TCP. Its own
addresses aren't in its offer; the media part learns the one it uses from
the connectivity checks, as it does a browser's. Linux builds need the ALSA
headers (`libasound2-dev`).

## Privacy

Apps only ever connect to the media part, never to each other: there's no
peer-to-peer fallback. Nothing is kept of anyone's camera or screen unless
their server records video and someone records the call, which everyone in
it sees. The media part never sends anyone another person's
addresses or candidates, and nothing about where someone connects from goes
in call events, logs or the audit log.

TURN credentials (coturn's REST scheme, from `FUWA_TURN_SECRET`) are new for
every connection, last an hour, and name no account: the username is the
expiry and a random name.

## What the media part accepts

An app's offer may send one track of sound, one of camera, one of screen
and one of the screen's sound, receive the
others' tracks and open the data channel, and nothing else; at most 10
offers in 10 seconds. Each person's sound is capped at 80 KB a second and
1500 bytes a frame, far above any Opus voice, and so is each program's.
Each camera is capped at 2 MB a second, all its sizes together (room for 1080p at 60), and 512 KB
a frame; each shared screen at 2 MB a second (room for 1440p at 60 and its
keyframes) and 512 KB a frame. A call holds at most 99 people, programs included.

## Hosting the media part

It needs one port reachable from the internet, UDP and TCP (`FUWA_MEDIA_PORT`,
50000 by default), and to know the addresses apps reach it on
(`FUWA_MEDIA_ADDRESSES`, its own by default). See
[self-hosting.md](self-hosting.md#calls).

Railway has no public UDP. There the media part goes behind a TCP proxy
(`FUWA_MEDIA_ADDRESSES=tcp/<proxy host>:<proxy port>`), so apps use ICE-TCP
on the proxy's random port. A TURN server can't help there: it relays to
the media part over UDP. For UDP and port 443, which strict networks still
allow, the media part runs on a host of its own and the shards reach it
over HTTPS with a media-only key ([self-hosting.md](self-hosting.md#the-media-part-on-a-host-of-its-own),
`deploy/media-host`). With a
volume-less media service, deploys overlap, so the restart above is the only
interruption.

### Cameras and screens on the desktop

The desktop app's offer has the same empty camera and screen places, each
with the three simulcast sizes (str0m's simulcast send layers, rids l, m
and h). Pictures are VP8 through libvpx (the `shiguredo_libvpx` crate: a
static libvpx, fetched prebuilt for each system and checked against its
SHA-256, so installers carry nothing extra; on Linux it's named for an
Ubuntu release, which `LIBVPX_TARGET` picks where the build machine isn't
one). Its Windows build is MinGW's, which MSVC can't link, so on Windows
it's vcpkg's libvpx through `env-libvpx-sys` (`voice/libvpx.rs`, set up as
the prebuilt one is; CI runs it on Linux with `--no-default-features
--features system-libvpx`). Cameras come through nokhwa (V4L2, AVFoundation, Media Foundation),
screens and windows through scap (X11, ScreenCaptureKit, Windows Graphics
Capture; not Wayland yet), each taken on a thread of its own and encoded on
another in three sizes at the web's sizes, frame rates and bitrates (CBR,
real time). Only the newest picture waits for the encoder, a full queue to
the connection makes that size wait for a keyframe, and keyframes are sent
when the media part asks (FIR, PLI) and whenever the connection is new.
macOS asks before the camera (`core/voice/access.rs`) and the screen; a
refusal says so with a way to System Settings. `FUWA_DESKTOP_FAKE_VIDEO=1`
sends a moving test pattern instead, for machines with neither.

Coming in, each feed gets a decoding thread that leaves only its latest
picture (as BGRA) for the window: one the window didn't take yet is written
over, its buffer reused, and a frame that can't wait is dropped and a
keyframe asked for. Each window says how tall it draws each feed as it
paints (only what's in sight), and the call asks for the size that fits, or
"off", as the web's `video.ts` does. Popped-out feeds are windows of their
own. The desktop doesn't share a screen's sound yet.
