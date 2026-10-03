# Shared channels

A shared channel is one text channel that shows up in two community servers.
It's like Slack Connect: two communities talk in one room while each keeps
its own server, roles and moderators. This is phase 1: servers on the same
instance, text and announcement channels, one other server per channel.
Voice, more servers per channel and sharing across instances come later.

## Who keeps what

- **The home** is the server the channel was made in. Its file holds the
  channel and every message said in it, wherever they were written from, so
  the channel lives in the home's region and under the home's storage limits.
  Its shard runs every write.
- **The guest** is the server the channel is shown in. It gets a channel of
  its own (`channel_links` in its file) with its own name, category and
  permission overwrites, and keeps no message from it: reads and writes go to
  the home, and what's said there comes back to the guest's people live.
- **Apps only talk to their own server.** A guest's member never calls the
  home: the guest's shard passes the call on (`App::shared` in
  `cluster/calls.rs`, through the directory when split), so nobody's app learns
  where the other server lives and no address is passed between them.

## Connecting

Both servers' admins agree, so neither can pull the other in:

1. Someone who can manage the home server and the channel makes a **share
   code** (`SharedChannelService.CreateShareCode`). It starts with the home
   server's id so the guest's shard knows where to ask, works for 7 days and
   lets one server ask. Codes are secrets like invites: audit-only, no event.
2. The guest's admin pastes it. `PreviewShare` shows which server holds the
   messages, the channel's name and topic and what the guest's people will be
   let do, before anything changes. `AcceptShare` asks the home, which records
   a waiting connection (`channel_guests`, inactive) and burns the code; the
   guest records the request (`channel_links`, no channel yet).
3. The home's admin approves (`ReviewShare`). The guest makes its channel
   first, so an approval it can't take (no room for another channel, say)
   never stands at the home; then the home marks the connection active.
   Turning it down ends the request on both sides. Once approved, the
   guest's people can read the whole channel, messages from before the
   share included; the Share tab says so before the home hands out a code.

Either side can **disconnect** at any time (`Disconnect`). The guest's
channel goes; the messages stay at the home, guests' messages included.
Deleting the channel at the home, or either server, ends its connections
too (`shared::take_channel`, `take_server`, `tell_ended`). A side that misses
the news lets go the next time it hears "gone" from the other.

## What each side controls

- **The home decides what guests may do**, at most send messages, embed links
  and attach files (`UpdateConnection`; `SHAREABLE` in `api/shared.rs`).
  Seeing the channel comes with being shown it. Guests get no other
  permission there: they can't ping @everyone, @here or roles, pin, or manage
  anything (`Access::guest`).
- **The guest decides which of its people see it and may write**, with its
  own roles and overwrites on its own channel. Its people need both: the
  guest's permission here and the home's allowance there.
- **Each side's AutoMod reads what that side's people write**: the guest's
  rules run on its shard before a message leaves, the home's run when it
  arrives. A home rule that would time someone out keeps a guest out of the
  channel instead, since they aren't the home's member.
- **Each side moderates its own people.** A guest's moderators (Manage
  Messages in their channel) can delete their own server's people's messages
  there; the home's moderators can delete anyone's. The home can keep a guest
  person out of the channel (`BlockFromChannel`, needs Kick Members). Members
  of the home are kicked or banned as usual instead.
- **Slow mode** is the home's.

## How messages travel

- **Reads** (`ListMessages`, `GetMessage` on a guest's channel) are passed on
  as `GuestList` and `GuestGet`. The home returns only messages people wrote
  (no join messages or AutoMod alerts), each with `Message.shared`: the
  author's profile and their server, since the author needn't be a member of
  the guest. The guest rewrites the server and channel ids to its own.
- **Writes** (`SendMessage`, `UpdateMessage`, `DeleteMessage`) are passed on
  as `GuestSend`, `GuestEdit` and `GuestDelete`. The home stores the author's
  profile (`users.guest_of` names their server) so its own members see who
  wrote it. @everyone, @here and role pings never reach the other server:
  guests' messages are stored without them, and messages shown to a guest
  have them cleared (`no_pings`). Edit and delete take `channel_id`; apps that leave it out still
  work, more slowly (`shared::locate` asks each connected home).
- **Live**: `spawn_shared_fanout` reads every event the shards holding servers
  publish (`Hub::shared_tap`), picks the messages created, edited and deleted
  in channels with guests, and sends each guest server its own ordered queue
  (`HomeEvents`). The guest publishes them to its members as sequence 0: shown,
  not kept, so a client catching up after a gap re-reads the channel from the
  home.
- `Channel.shared` tells apps a channel is shared: from here with which
  servers (`home`), or from which server and channel (not `home`).
  `SharedChannelsUpdated` tells managers to re-read `ListConnections`.

## What can't be shared

- Voice channels, categories, threads and forums (phase 1).
- End-to-end encrypted channels and direct messages, ever: the home server
  stores the messages, so there'd be no end-to-end left.
- A channel shown from another server can't be shared on, take webhooks,
  join messages or AutoMod alerts (they'd have nowhere to live).

## Turning it off

`FUWA_SHARED_CHANNELS=off` (or the toggle in the instance settings) stops new
codes, previews and requests. Connections already made keep working until an
admin disconnects them.

## Later

- Voice in shared channels, with guests agreeing before a recording.
- More than one guest server per channel.
- Across instances (phase 2): the home instance relays everything, instances
  trust each other through signed instance keys, off by default.
- An instance admin control to end any connection, the region in the preview
  once regions land, and the desktop app's screens.
