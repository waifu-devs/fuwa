# Read marks

Read a channel on your phone and it stops showing unread on your computer
too. Each app says how far you've read with `AccountService.MarkRead`; the
instance keeps the mark on your account and passes it to every live
connection of yours that asked for `reads` (docs/live.md), so their unread
counts and badges clear. Apps check the `read-marks` feature before using
either ([compatibility](compatibility.md)).

## Places

A mark is for one place, by id:

- a server channel (`message_id`: the newest message read),
- a thread, by the id of the message it's under (`message_id` too),
- a direct message conversation (`sequence`: the newest record read),
- a secure channel, by the channel's id (`sequence`), and a thread in one as
  `<channel id>/<sequence of the record it's under>`.

A channel or thread is usually marked with an empty `message_id`: everything
up to now. The instance fills in an id that sorts after every message sent
before it (message ids are ULIDs), so apps needn't know a channel's newest
message to mark it, and a mark from a shard and one from the directory agree
to within the parts' clocks.

Marks only move forward: one behind what's kept changes nothing and isn't
passed on. MarkRead answers with each place as kept now. The instance never
checks that you can see a place: a mark only ever reaches your own devices.
It keeps up to 20,000 places per account and forgets the ones longest
unmoved beyond that (those just show unread again); marks are in your data
export and go with your account.

Encrypted conversations and secure channels are marked by record sequence,
which the instance already knows; what was said stays unreadable to it.

## On the live connection

`OpenRequest.reads` asks for the marks. With `reads_after` set to the newest
`at_ms` the app heard, the connection first sends every mark taken since
then, with `ready`; after that each mark as it's taken, this device's own
included. An app opening for the first time sends 0 and hears only what
comes from then on: it has no unread counts from before yet. On a split
instance marks are kept and sent by the directory; the gateway passes them
through.

## In fuwa's apps

The web and desktop apps mark:

- the server channel and thread on screen, when they come on screen, when
  someone else's message arrives in them, and when you come back to the app
  (only while it's in front: a tab in the background reads nothing);
- channels you mark read by hand (Mark as read on a channel, category or
  server);
- conversations and secure channels (and their threads) whenever the device
  notes that it read further, as it already did for its own unread counts.

They gather marks for a moment and send them together, so a busy channel on
screen sends one now and then rather than one per message.

A mark heard clears that place's unread count, and messages and channel
heads at or before it don't count as unread again. For conversations and
secure channels the app notes the record as read, as if read on that
device, and counts again from there.
