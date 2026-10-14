# Typing indicators

While someone writes a message, the people looking at the same channel,
thread or conversation see a line above their composer: "Ana is typing",
"Ana and Ben are typing", "Ana, Ben and Cy are typing", and past three,
"Several people are typing". Apps check the `typing` feature
([compatibility](compatibility.md)) before sending or showing any of it.

## On the wire

- In a server: `MessageService.SendTyping` with the channel, and the
  thread's id when typing in a thread's reply box. It needs Send Messages
  in the channel. The instance publishes a `TypingStarted` event (channel,
  thread, who) with sequence 0, like a voice state: it isn't stored, isn't
  replayed when catching up, and moves no cursor or channel head. A
  secure channel's thread, which has no message id the instance knows,
  is named by its parent record's sequence, written in digits.
- In a direct message: `DirectMessageService.SendConversationTyping`, and
  the others in the conversation get `DirectMessageEvent.typing` on their
  direct-message feed.

Nothing typed travels with either, so secure channels and encrypted
direct messages have typing too: the instance learns only that someone is
typing, which it could tell from the message arriving anyway.

## Who hears it

- A server's typing goes to the members who can see the channel, as every
  channel event does. On a live connection ([live.md](live.md)) it comes
  only for the channels in focus (or to agents that asked for every
  message), and is dropped for the rest: it's worth nothing a few seconds
  later, so it never becomes a head.
- In a channel shared between servers, it reaches the members of the
  typist's own server only.
- In a direct message, someone who blocked the typist never hears of it,
  and the typist isn't told ([friends.md](friends.md)); nor does typing go
  to someone the typist blocked.
- Agents send it like anyone (`agent.typing`, `typingWhile` in the SDK,
  `send_typing` over MCP) and get `TypingStarted` on their streams. Agent
  endpoints don't: they get only events in the log.

## Timing

- Apps send a signal when the person edits the composer's text, at most
  every 6 seconds per place, and again straight away after they send.
- The instance passes on one signal per person and place every 3 seconds
  and quietly drops the rest, so a burst of keystrokes from a modified app
  fans out once (`server/src/typing.rs`). It's kept in memory only.
- Apps show someone as typing for 10 seconds after their latest signal, or
  until a message from them arrives in that place, and never show
  themselves.

## Turning it off

"Show when I'm typing" (on the Privacy page, beside "Show what I'm doing";
the `sendTyping` app setting, which travels with the other app settings:
[app-settings.md](app-settings.md)) stops the app sending any signal.
Others' typing still shows.
