# Reactions

People react to messages with emoji: a standard emoji, or one of the
server's custom emoji. Every app shows them as a row of chips under the
message, each with its count, the reader's own marked.

## In channels and threads

`MessageService.React` adds the caller's reaction or takes it off;
`ListReactors` says who reacted with one emoji, the earliest first;
`ClearReactions` takes one emoji's reactions off a message, or all of them.

- **Storage**: one row per person, emoji and message in the server file's
  `reactions` table (`server/src/api/reactions.rs`). A standard emoji is
  stored as its characters, a custom one by its id; the two never look alike,
  since standard emoji have no letters. Variation selectors (U+FE0F) are
  left out of the stored emoji, so "👍" and "👍️" are one reaction, shown as
  its first reactor wrote it (`shown`); apps match chips the same way. Reads
  add each message's up
  (`reactions::attach`) in the order each emoji was first used, with `me`
  for the reader, and skip custom emoji the server has deleted since.
- **Who may**: reacting needs `ADD_REACTIONS` in the channel (given wherever
  Send Messages was when it arrived, and to @everyone in new servers; held
  back from members who haven't agreed to the rules and from those timed
  out). Taking your own reaction off needs only to see the channel.
  `ClearReactions` needs Manage Messages and writes an audit entry
  (`REACTIONS_CLEAR`, the target being the message's author).
- **Events**: `ReactionUpdated` (who, the emoji, its new count, added or not)
  and `ReactionsCleared`. Events never say `me`: apps compare `user_id` with
  their own account. Each reaction rewrites its message's row, so two at once
  clash and one runs again: counts in events follow each other, and the cap
  holds. Events that carry a whole message (`MessageCreated`,
  `MessageUpdated`) have no reactions; apps keep the ones they had.
- **Caps**: `reactions_per_message`, the most different emoji on one message,
  is unlimited unless an admin sets it (`FUWA_LIMIT_REACTIONS_PER_MESSAGE`).
  More people reacting with an emoji already there is always fine.
- **Going away**: deleting a message, its thread, its channel, or a ban's
  purge takes the message's reactions along. Deleting an account takes its
  reactions off, with an event for each count that went down. Leaving a
  server keeps them.
- **Shared channels**: the home keeps every reaction, its guests' too
  (docs/shared-channels.md). A guest's reaction goes as `GuestReact` and
  "who reacted" as `GuestReactors`; the home checks its connection lets
  guests add reactions (`ADD_REACTIONS` is shareable), turns away people it
  kept out, and its events reach the guest like message events. Guests react
  with standard emoji or the home's own (`Reaction.emoji_url` carries the
  picture, through the guest's instance when it's another one); only the
  home clears reactions. Reactions from before the share show to guests, as
  the channel's history does.

## In direct messages and secure channels

The server never learns who reacted to what there. A reaction is a
`DirectMessageReaction` inside the encryption (`DirectMessageContent`, sent
signed in a secure channel like a text): the record reacted to, a standard
emoji, and whether it's taken off. Each device tallies what it reads, the
latest from a sender for an emoji winning; reactions never show as messages,
count as unread or notify. `React` refuses secure channels so nothing reaches
the server in the clear.

## Agents

The SDK's `agent.react(message, emoji)` and `agent.reactors(...)`
([sdk.md](sdk.md)) and the MCP tools `react` and `list_reactors`
([mcp.md](mcp.md)) wrap the same calls.
