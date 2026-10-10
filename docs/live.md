# Live connections

An app hears everything from an instance on one stream, `LiveService.Open`,
instead of one stream per feed (server events, direct messages, friends,
presence). It tells the instance what it shows with `LiveService.Focus`, and
in busy servers the instance then sends whole only what's on screen.

Old apps keep their separate streams (`EventService.Subscribe`,
`DirectMessageService.Watch`, `FriendService.WatchFriends`,
`PresenceService.WatchPresence`); nothing about them changes. Apps check the
`live-connection` feature before using this ([compatibility](compatibility.md)).

## The stream

`Open` takes the same server cursors as `Subscribe` and says which other
feeds to carry. It sends first `connection_id`, then each feed's items in the
shapes its own stream uses, so an app's handlers carry over. A response with
nothing set is the one heartbeat for the whole connection. Any feed's error
ends the stream; the app opens it again.

## Focus

`Focus` names the connection and what the app shows: up to 8 channels and
up to 500 people (member list rows, authors on
screen). Each call replaces the last, and calls in quick succession fold
together (the latest wins), so apps needn't hold back while scrolling. The
stream echoes the focus in effect as `focus`; from the echo on, those
channels' messages come whole. An app opening a channel calls Focus, waits
for the echo, then loads the channel's messages: nothing falls in between.
`Open` takes a `focus` too, in effect from the start, so catching up from a
cursor already sends the channel on screen whole.

Only the session that opened a connection can focus it; any other id is
NOT_FOUND, whoever's it is. Signing that session out ends the stream at once,
and its id stops resolving.

## What comes whole

Server structure (channels, roles, members, voice, live tiles, the server
itself) comes to every connection, as on `Subscribe`. Messages, reactions,
polls, pins and thread changes come whole only:

- in the channels in focus,
- when a message mentions the caller (by `@username` or `<@id>`, a role
  they have, or everyone),
- for the caller's own messages, sent from any device,
- always for agents, unless they ask for `MESSAGE_INTENT_MENTIONS`.

A thread's replies come with its parent channel, so focusing the channel
covers its threads. Mentions count for new and edited messages; a deleted
message moves only the server's sequence. On a split instance the gateway
doesn't know the caller's roles, so a message mentioning any role the caller
can see comes whole.
Nothing comes, whole or as a head, from a channel the caller can't see.

The rest move `heads`: at most every 2 seconds, for each server, the newest
message id of each channel with new messages (only channels the caller can
see; a reply only in its thread moves just the sequence) and the sequence a
cursor may move to. Heads owed for a server always go out before a later
event of that server comes whole, so a cursor moved to that event never
skips them. Catching up from a cursor follows
the same rule, so a reconnect after a busy hour replays only what was in
focus, and the heads before `ready` say where everything else stands. An app
moves a server's cursor to the higher of its last event's sequence and its
last heads' `sequence`, and loads a channel's messages when it opens it.

On a split instance the gateway holding the stream merges the shards' events
and holds back what's out of focus; the directory keeps the focus (Focus can
reach any gateway) and tells that gateway over the connection's own stream.

## In fuwa's apps

The web and desktop apps open a channel without waiting for the echo; when
the echo puts a channel in focus (and after each reconnect), they read its
newest page again and merge it, replacing what they had if the two don't
meet. A head lights a channel's unread mark; for channels set to notify on
every message, the app reads what the head covers and notifies from that.
Heads don't say which thread moved, so threads someone follows light up only
when their channel is in focus or the reply mentions them.

## Presence

A connection's presence is what's on screen: everyone online in servers with
fewer members than `FUWA_LARGE_SERVER_MEMBERS` (2,500 unless the instance says
otherwise), and the people in focus who share any server with the caller.
Smaller servers are followed smallest first, up to `FUWA_ON_SCREEN_MEMBERS`
members in all (5,000 unless the instance says otherwise); past that a
server counts as large for that connection, so someone in hundreds of
servers still holds a bounded stream. Servers are chosen when the stream
opens and as the caller joins or leaves them; a followed server that later
grows past the large size may miss its people out of focus until the app
opens a new stream. Whether someone is shown is worked out each time they
change, from the servers shared then.
Someone coming into focus is sent at once if they're online; someone going
out of it gets no more updates. Leaving the last server shared with someone
shown sends them offline. In a large server, a person coming online then
costs the connections that have them on screen, not one message per member.

Old apps' `WatchPresence` streams are unchanged: everyone in every shared
server, so in large servers they keep today's cost (and a slow reader's
stream is still cut) until they move to live connections.
