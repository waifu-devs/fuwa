# Threads

Replying to a message in a text channel can start a thread under it: a
conversation of its own beside the channel, so a side discussion doesn't
fill the channel. Inline replies (`reply_to_id`) stay as they were; a thread
is a separate thing.

## Model

A thread reply is an ordinary message in the channel with `thread_id` set to
the id of the message it's under (the parent). That's the whole model, so
AutoMod, slow mode, mentions, permission overwrites, edits and deletes treat
replies like any other message. Replies don't nest: a thread can't start
under a reply, and only messages people wrote (not join messages or AutoMod
alerts) can have one.

- `messages.thread_id`, `messages.in_channel`: a reply, and whether its
  author also sent it to the channel ("Also send to #channel"). The channel's
  own list shows messages without a thread and replies with `in_channel`.
- `threads`: one row per thread, keyed by the parent's id, summing it up for
  the channel to show (`ThreadSummary`: reply count, last reply, the five
  latest people to reply, locked). It's recomputed in the same write as any
  reply's send or delete and published as `ThreadUpdated`, which only people
  who can see the channel receive.
- `thread_follows`: who follows which thread. Replying, or writing the
  parent, follows it; following or unfollowing by hand sticks. Apps notify
  for a followed thread's replies and for mentions, and count unread replies
  only in threads you follow.

## Permissions

- **Start threads** (`PERMISSION_CREATE_THREADS`) lets someone reply to a
  message that has no thread yet. Replying in an existing thread needs only
  **Send messages**. The migration gives the new permission to every role and
  overwrite that had Send messages, so nothing changes until a server says
  so.
- **Manage messages** locks or unlocks a thread (audit `THREAD_LOCK`,
  `THREAD_UNLOCK`); a locked thread takes replies only from people with it.
- Deleting a parent deletes its thread with it. Its author may do that only
  while no one else has replied; after that it takes Manage messages (audit
  `THREAD_DELETE`).

## Archiving

A thread with no reply for the server's `thread_archive_hours` (a week by
default, 0 to never archive, at most a year) is archived. Nothing runs to
archive it: ListThreads compares the last reply's time with the setting, and
the apps show "Archived". A reply brings it back.

## Listing and search

`ListThreads` pages a channel's open or archived threads, latest reply
first. A search looks through the 500 most recent threads in that list,
matching the parent and up to 1000 replies of each, and reads at most 5000
replies per request; each account may search 30 times a minute (counted per
account, never per address).

## Secure and shared channels

- Shared channels: threads work on both sides and are kept at the home,
  like the channel's messages: summaries, replies, follows and searches
  (see [shared-channels.md](shared-channels.md)). Only the home's
  moderators lock them. Polls stay out of threads there.
- Secure channels don't use MessageService. Their threads live inside the
  encryption and the devices do what the server does here: see
  [secure-channels.md](secure-channels.md#threads).
