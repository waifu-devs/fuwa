# Presence

Who's online and what they're doing, like Discord's status dots and rich
presence, and compatible with the games and apps that already report to
Discord.

## What people see

- A dot on every avatar: online, idle (a moon), do not disturb (a dot with a
  bar) or offline (a ring). Invisible looks offline to everyone else, friends lists included.
- Your own status from the menu on your name at the bottom of the sidebar:
  online, idle, do not disturb (no sounds or notifications from that
  instance) or invisible. The custom status ("at the gym") is the one on
  your profile.
- What someone is doing ("Playing Celeste", "Listening to …"), with details,
  a live timer, party size, pictures and up to two buttons, on their profile
  card and as one line under their name in the member list. Members who are
  online are listed first; everyone offline comes last, faded.

## Who sees what

- Your presence reaches only people who share a server with you (friends
  too, once there are friends). Nobody else, even with your id.
- Your status shows to them unless you're invisible.
- What you're doing shows only once you turn on "Show what I'm doing"
  (Settings → Data and privacy). It's off for everyone until they do. You
  can then turn it off in single servers ("Share my activity here"): someone
  sees it if any server you share with them isn't hidden.
- Admins can turn rich presence off for the whole instance
  (`FUWA_RICH_PRESENCE`, or Instance settings → Rich presence). Activities
  shown are taken down at once and new ones dropped as they arrive; statuses
  still show.

## How it works

`PresenceService` (proto/fuwa/v1/presence.proto) is served where accounts
are: a single process, or a split instance's directory (gateways pass it
through like `DirectMessageService`).

- Each signed-in app calls `UpdatePresence` when it starts and every minute,
  saying whether its person is away from it (the web app: no input for ten
  minutes) and what they're doing there (the desktop app: what games tell
  it). An app's word lasts 150 seconds.
- A person with no app is offline. They're idle when every app of theirs
  says so (or they picked idle). What they're doing comes from their most
  recent app that's doing something.
- `WatchPresence` streams everyone you can see who's online, then `ready`,
  then each change; your own presence comes too, so your other apps follow
  your status. A join on any shard introduces the newcomer and the server's
  members to each other (`Presence::joined`, called where the index learns
  of the join).
- Changes go out at most 5 times per 20 seconds per person; more are merged
  and the latest goes out when the window opens. Nothing is refused, so a
  game updating every second costs its viewers nothing extra.
- Each viewer is sent someone's presence only when what *they* see of it
  changes. An invisible person's apps coming and going, going idle or
  starting a game send nothing at all, and neither does an activity hidden
  from the servers a viewer shares.
- Leaving a server (or being removed, or the server being deleted) shows
  people who no longer share one as offline to each other at once.
- One account holds at most 16 streams (a tab or app each); a 17th ends the
  oldest, whose app watches again if it's still open. Streams give their
  place back however they end.
- Agents report their own presence but can't watch anyone's.

What someone is doing lives only in memory (`server/src/presence.rs`): it's
never written to disk, logged, put in a server's event log or in a report. A
restart forgets it, and apps send it again within a minute. Only the status
you pick and the two switches are stored (node.db's `presence_settings`).

## What an activity may hold

Every field is the person's own content, checked as it arrives:

- name 1 to 128 characters; details, state and picture texts at most 128;
  control characters become spaces, and direction overrides and
  zero-width characters are removed (the zero-width joiner stays, for emoji);
- party size and maximum at most 1,000,000;
- at most 2 buttons: a label of 1 to 32 characters and an `https` link of at
  most 512 characters with no user name or password. Apps open them only
  after a confirmation naming the host;
- at most 5 activities;
- an application id (the Discord client id a game sent) of letters and
  digits, kept only to label the activity.

### Pictures

No app ever loads a picture from anywhere but its instance. An activity's
picture sent as an `https` link becomes a link the instance fetches itself
(`App::picture_link`, as for embeds). One account gets at most 30 new
links signed an hour, so the fetcher can't be used as anyone's proxy; past
that its pictures are dropped until the hour is up. Discord's own pictures are asset keys
that point at Discord's CDN, which we never load: they're dropped, and apps
show the activity's kind (a gamepad, headphones…) in the theme's color
instead. A list of app pictures kept on the instance (uploaded by admins,
later suggested by the community) is planned, so well-known games get their
artwork back without anyone contacting Discord.

## Discord compatibility (desktop app)

Games and apps report what you're doing to Discord through its local RPC.
The desktop app listens in the same places, so an unmodified game that uses
discord-rpc, the Discord Game SDK or pypresence shows up in fuwa
(`desktop/src/core/presence/`):

- Where: the Unix socket `discord-ipc-0` to `-9` in `$XDG_RUNTIME_DIR` (else
  `$TMPDIR`, `$TMP`, `$TEMP`, `/tmp`, as the libraries search), or the
  Windows pipe `\\.\pipe\discord-ipc-N`, taking the first free number so a
  running Discord keeps its own. A socket left behind by you is replaced;
  anyone else's is left alone.
- Who: only your own programs. The socket is made 0600 and each connection's
  user is checked; Windows pipes refuse other computers, and their default
  security lets other users read but not write.
- What it speaks: the handshake, then `SET_ACTIVITY` (or null to clear it),
  `SUBSCRIBE` and `UNSUBSCRIBE` (answered, nothing is ever sent), PING and
  CLOSE. Anything else gets an error with a fixed message. READY carries a
  placeholder user, never anything about you. Frames over 64 KiB end the
  connection, 16 programs connect at once, and a program has 10 seconds to
  say hello.
- Asking: the first time a program connects you're asked whether it may
  show what you're playing, naming its process (on Windows, which doesn't
  say, the Discord id it gave). Allow and Don't allow are remembered; closing
  the question asks again the next time the app starts. Privacy settings
  list every answer with "Ask again", and "Show what you're playing" turns
  the listening off.
- What goes out: allowed programs' activities, newest first, one per game
  and at most five, named after the program (Discord names them from its
  own application pages). Text is cut to fit what instances take, links must
  be https, and Discord's picture keys are left out. The desktop app updates
  its presence on each instance every minute and two seconds after a game's
  activity changes; your presence settings there decide who sees it.
