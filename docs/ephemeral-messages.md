# Ephemeral messages

An agent can send a message in a channel that only one person there sees,
for a while: a private answer to a command, a hint, an error nobody else
needs. Apps show it among the channel's messages with "Only you can see
this", how long it has left, and "Dismiss message", and drop it when it runs
out. It's Discord's ephemeral message, sent by agents.

## On the wire

- `MessageService.SendEphemeralMessage`: the server, channel, who sees it
  (`user_id`), the text and embeds, and how long it shows
  (`expires_in_seconds`, up to an hour, 15 minutes unless given).
  - With `interaction_id` it answers a command or a button privately: it
    goes to whoever used it (`user_id` may be left out, and must be them if
    given), counts toward the interaction's 5 answers within 15 minutes, and
    shows who used what, like an answer sent with `SendMessage`.
  - Only agents send them, with Send Messages in the channel (and Embed
    Links for embeds), not while timed out. At most 60 a minute from one
    agent, kept in memory (`server/src/api/ephemeral.rs`).
  - Only in the server's own text and announcement channels: not in
    threads, secure channels or channels shared between servers.
  - The person must be a member who can see the channel, and a person, not
    an agent. Someone who isn't there looks the same as someone who doesn't
    exist (`NOT_FOUND`).
- The instance stores nothing. It publishes one `EphemeralMessageCreated`
  event (the whole `Message`, with `Message.ephemeral` saying for whom and
  until when) with sequence 0, so it isn't in the log, isn't replayed when
  catching up, and moves no cursor or channel head. `ListMessages`,
  `GetMessage`, search and exports never have it.
- It pings nobody: no mentions, notifications or unread marks, though
  `mention_user_ids` is filled in so apps can draw names.
- No files, polls, buttons, reactions, replies or edits: there's nothing
  stored to press, react to or change.

## Who gets it

Only the one person, on every live connection of theirs that follows the
server, whatever is in focus (`events::shown_to`, and `Interest::take`
passes it whole): it's addressed to them, so it isn't held back as a head.
They must still see the channel when it goes out. Someone not connected
never gets it; closing or reloading the app drops it. Agent endpoints and
MCP never carry it, since it's never for an agent.

## In the apps

The web app (`web/src/fuwa/ephemeral.ts`, `web/src/lib/ephemeral.ts`) and
the desktop app (`desktop/src/core/ephemeral.rs`) keep each one in memory
under its channel until `expires_at`, and the message list draws it after
the messages before it, tinted, with "Only you can see this", the minutes
left and "Dismiss message". It never counts as unread and is never written
to a cache.

## For agents

The SDK has `agent.sendEphemeral(serverId, channelId, userId, content)`,
`ctx.replyEphemeral(content)` on a message (to its author) and on an
interaction (to whoever used it). Over MCP it's `send_ephemeral_message`.
An endpoint's answer to an interaction is always an ordinary message; call
`SendEphemeralMessage` with its `interaction_id` instead to answer privately.
Instances say they have it with the `ephemeral-messages` feature.
