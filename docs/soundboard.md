# Soundboard

Short sounds a server keeps, which anyone in one of its voice channels can
play for everyone in the call: an air horn, a laugh, a "gg". Discord calls it
the soundboard, and fuwa's works the same way: the sound never goes through
the call's media. Each app hears that someone played it and plays the file
itself.

## Sounds

A server's sounds are rows in its file (`sounds`), served by
`SoundboardService` (`server/src/api/soundboard.rs`):

- **ListSounds**: every sound, oldest first, for any member.
- **CreateSound**, **UpdateSound**, **DeleteSound**: adding, renaming
  (or changing the emoji or volume) and deleting, with Manage Soundboard
  (`PERMISSION_MANAGE_SOUNDBOARD`, server-wide, given wherever Manage Emoji
  was by migration 0044 and part of a new server's Admin role). Each one is
  audited (`AUDIT_ACTION_SOUND_*`) and sends the whole list to members as
  `SoundsUpdated`, like emoji.

A sound has a name (1 to 32 characters, unique in the server whatever their
case), an emoji shown beside it (a Unicode emoji or one of the server's own,
written `<:name:id>`, or none), a volume from 0 to 1 that it plays at, how
long it plays and its file.

The file is an upload like an emoji's picture (`MEDIA_PURPOSE_SOUND`, for
the server, under the attachment upload caps), counted in the server's
attachments, and deleted with the sound. It's always Ogg Opus: the instance
checks the bytes when they arrive (`media::receive_file`), reads how long it
plays from its pages (`media::ogg_opus_ms`: the last granule position less
the pre-skip) and refuses one past 10 seconds (`MAX_SOUND_MS`), or one that
isn't one stream of Opus. A file over 1 MiB can't be 10 seconds of Opus, so
it's refused before it's sent. Apps turn whatever the person picks (MP3,
WAV, Ogg, FLAC, M4A...) into Ogg Opus on the device, cut to the part they
chose of at most 10 seconds, and send how long it is with CreateSound.

## Playing one

**PlaySound** says which sound, in which voice channel. It takes being in
that channel's call (a place in `App::voice`, so programs listening with
ListenVoice can play them too), Use Soundboard (`PERMISSION_USE_SOUNDBOARD`,
per channel, given wherever Speak was by migration 0044, and in @everyone in
new servers) and Speak there, and not being muted or deafened, by yourself or
a moderator: nobody plays a sound they couldn't say. A member who hasn't
agreed to the rules can't (`permissions::TALK`), nor one who's timed out.
How many one account may play a minute is the instance's
`sound_plays_per_minute` (`FUWA_LIMIT_SOUND_PLAYS_PER_MINUTE`, on the
Limits page), unlimited unless set.

It's then published like a voice state, not stored: a `SoundPlayed` event
with sequence 0, scoped by its channel (`events::channel_of`), so only
members who can see the channel get it, and live connections get it whole
whatever their focus. It carries the sound as it is, so an app plays it
without listing the sounds first.

An app that gets one plays it only if it's in that channel's call, and not
deafened: the file (fetched once and kept for the session), at the sound's
volume times the person's own Soundboard volume (Voice settings,
`soundboardVolume`, 0 to 1, synced like the other voice settings), through
the call's output device. Sounds others play overlap rather than cut each
other off. An app shows who played what for a moment beside their tile.

What the apps don't do: sounds aren't mixed into anyone's microphone, so
programs listening (ListenVoice) and recordings on the server don't hear
them; a recording on your own device does, since it records what you hear.
Direct-message calls have no soundboard: they're end-to-end encrypted, and a
sound played there would have to go through the server in the clear.

## In the apps

- **The soundboard button** sits with the call's controls (mute, deafen,
  camera, screen) in a voice channel, for people with Use Soundboard there.
  It opens a grid of the server's sounds, each with its emoji and name;
  pressing one plays it for everyone (and you hear it like everyone does),
  and a small play button beside it previews it for you alone. While you're
  muted or deafened the grid says why it can't play.
- **Server settings, Soundboard** (Manage Soundboard): the sounds, each
  with a preview, its name, emoji and volume, edited in place, and deleted
  with a confirmation. Adding one picks a file, shows how long it is with
  a 10-second window to keep (the start can be moved), a name (from the
  file's), an emoji and a volume, and uploads it.

The web app's side is `web/src/fuwa/soundboard.ts` (the sounds per server,
following `SoundsUpdated`, and playing `SoundPlayed`), `web/src/lib/soundboard.ts`
(pure: whether a played sound is heard here, and the volume),
`web/src/voice/sound-file.ts` (any audio file to a cut of Ogg Opus, through
WebAudio and WebCodecs, written by `voice/ogg.ts`),
`components/calls/Soundboard.tsx` and `settings/server/Soundboard.tsx`. The
desktop's is `core/soundboard.rs` (the same), `ui/soundboard.rs` and
`ui/server_settings/soundboard.rs`; it decodes files through Symphonia and
encodes with libopus, as voice messages do.
