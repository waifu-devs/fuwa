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
  Its shard runs every write. On an instance with more than one region, the
  preview names the home's (`PreviewShareResponse.region`) before the guest's
  admins ask.
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

Instance admins can end any of them too: the Servers page in Instance
settings lists each server's shared channels, both ends (`ListServerShares`),
and **End** works like that server's own Disconnect (`EndServerShare`). Its
audit log notes an instance admin did it.

## What each side controls

- **The home decides what guests may do**, at most send messages, embed links,
  attach files, create polls, start threads and add reactions
  (`UpdateConnection`; `SHAREABLE` in `api/shared.rs`). A new connection
  starts with all of them; one made before polls could cross keeps what it
  had until the home's admins turn Create polls on. Connections from before
  reactions got Add Reactions wherever they let guests send messages.
  Seeing the channel comes with being shown it. Guests get no other
  permission there: they can't ping @everyone, @here or roles, pin, or manage
  anything (`Access::guest`).
- **The guest decides which of its people see it and may write**, with its
  own roles and overwrites on its own channel. Its people need both: the
  guest's permission here and the home's allowance there.
- **Each side's AutoMod reads what that side's people write**: the guest's
  rules run on its shard before a message leaves, the home's run when it
  arrives. A home rule that would time someone out keeps a guest out of the
  channel instead, since they aren't the home's member. Provider rules
  (docs/automod.md) count too: each side asks its own provider before its
  write, as for its own channels. The preview names the home's (`Name
  (host)` in `checked_by`) before the guest's admins ask, and a guest the
  home kept out is turned away before anything goes to its provider, as is
  anything the home doesn't let guests send (pictures without Attach Files
  or Embed Links). Both sides' Shared channels pages list the home's
  providers on each connection (`SharedConnection.checked_by`); a guest's
  page asks the home each time it loads, so a rule added after approval
  shows up there.
- **Each side moderates its own people.** A guest's moderators (Manage
  Messages in their channel) can delete their own server's people's messages
  there, and end their polls; the home's moderators can delete anyone's, and
  end any poll. The home can keep a guest
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
- **Files** a guest sends are the home's, like everything else in the
  channel. The guest uploads them for its own server as usual; the home
  checks each one is the sender's own upload for that server (and that they
  may attach files there), counts it against its own room for files, and
  takes it (`take_files` in `api/shared.rs`): it claims the upload's row
  first (`Node::take_media`, so a file goes in one message), then keeps the
  bytes with its own pictures. On a split instance its shard moves them in
  place, or copies them from the guest's shard (`ShardService.SendSharedFile`,
  checked by SHA-256 and the row's size) when the two servers live apart; in
  one process they stay where they are. The guest's shard then lets go of
  its copy. From then on the file is served, counted and deleted as one of
  the home's. From another instance, the home fetches them with a ticket
  (docs/federation.md).
- **Reactions** are the home's too, like polls (docs/reactions.md): a
  guest's go as `GuestReact`, "who reacted" as `GuestReactors`, and the home
  checks Add Reactions on the connection and who it kept out.
  `ReactionUpdated` and `ReactionsCleared` go to guests like message events.
  Guests react with standard emoji or the home's custom ones, never their own
  server's, and only the home clears reactions.
- **Polls** in a shared channel are the home's too: the poll, its counts and
  its votes are kept where the channel lives, and nothing of them on the
  guest's side. A guest's poll goes as `GuestPoll`, its votes as
  `GuestVote`, an early end as `GuestEndPoll` and "who voted" as
  `GuestPollVoters`; `VotePoll`, `EndPoll` and `ListPollVoters` take
  `channel_id` for a channel shown from another server (anywhere else it
  must be the poll's own channel, or empty). The home checks the poll is in
  the connection's channel and not in a thread, and turns away someone it
  kept out. Everyone in the channel, on both sides, votes and sees the counts
  live (`PollUpdated` goes to guests like message events), and "who voted"
  names people from both sides as members of the other already see them.
  A channel with polls running can be shared; a public poll's earlier
  voters then show to the guest's people too, as the channel's history
  does. Anonymous polls stay anonymous to everyone: their events name no
  one and their voters are never listed. The guest's own instance passes on
  its person's vote, so its operator could see that pick in passing, as it
  can see anything its people do there; the home's operator holds the key,
  as for its own polls. When someone's account is deleted on their own
  instance, their votes at another server's home stay, as their messages
  there do.
- **Custom emoji** show on both sides. The home adds its own server's
  emoji to each message it hands a guest (it doesn't keep them with the
  message, since its own people have them), so one deleted since shows as
  its name, as at home. A guest's own server's emoji go with what they
  write, read from the guest's server (never another server they belong
  to), and the home keeps them with the message, only as pictures of an
  upload on this instance. In a channel shown from another server,
  `ListEmojis` with its `channel_id` gives the home's emoji for the
  picker (`GuestEmojis`); the app offers those and the guest server's own.
  Writing the home's emoji needs nothing more: the home shows its own.
- **Threads** in a shared channel are the home's too: replies, summaries,
  who follows what, and archiving by the home's setting. A guest's reply
  goes as `GuestReply` (a `GuestSend` with the thread), a thread's page as
  `GuestThread`, the channel's threads as `GuestThreads`, following as
  `GuestFollow`, and the threads they follow there as `GuestFollowed`
  (which `ListFollowedThreads` asks each home for at once, waiting at most
  five seconds). Starting a thread needs Start threads from both the
  guest's own server and the home's grant (`PERMISSION_CREATE_THREADS`
  joins what a home can let guests do). Only the home's moderators lock a
  thread, and a locked one takes no guest's reply; a guest's message that
  others replied under can then be deleted only at the home. ThreadUpdated
  reaches guests like message events, and their apps decide notifications
  from it and the threads they follow. A thread search in a shared channel
  goes to the home's instance, search text included, since the home holds
  the threads and counts the search (30 a minute for each person). When a
  connection ends, its people's follows there go with it. Polls stay out
  of threads in shared channels.
- `Channel.shared` tells apps a channel is shared: from here with which
  servers (`home`), or from which server and channel (not `home`).
  `SharedChannelsUpdated` tells managers to re-read `ListConnections`.

## What can't be shared

- Voice channels, categories and forums (phase 1).
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
  trust each other through signed instance keys, off by default. The link
  between instances, and asking for and approving a share across it, are in
  [federation.md](federation.md), with messages and live events across it.
- The desktop app's screens.
