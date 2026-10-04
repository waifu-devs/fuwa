# Slash commands and buttons

How people use agents in fuwa: an agent gives each server it's in a set of
slash commands, people run them from the composer, and the agent answers
with an ordinary message that can carry buttons. Instances that have this
list `agent-commands` in `Node.versions.features`.

Everything here is `fuwa.v1.CommandService` (`proto/fuwa/v1/command.proto`)
plus two fields on `MessageService.SendMessage`. MCP agents get the same
through tools ([docs/mcp.md](mcp.md)).

## What people see

- Typing `/` at the start of the composer lists the commands of the agents in
  the server: the name, what it does and which agent it belongs to. Picking
  one swaps the box for its options, and Enter (or the send button) runs it.
  Escape goes back to typing.
- The agent's answer is a message in the channel headed "*Juan* used
  **/roll**".
- An agent's message can carry up to 5 rows of up to 5 buttons. Pressing one
  tells the agent who pressed which button; a disabled one can't be pressed.
  Link buttons open an `https` link in a new tab and tell nobody.

## For agents

### Setting commands

`SetCommands(server_id, commands)` replaces the calling agent's commands in
one server it's a member of (people can't set commands). Up to 50 commands:

- `name`: what people type after `/`, 1 to 32 of `a-z 0-9 _ -`, unique for
  this agent in this server.
- `description`: 1 to 100 characters.
- `options`: up to 10, required ones first, each with a `name` (like a
  command's), a `description`, a `type` and `required`:

| Type | People give | The agent gets, as text |
| --- | --- | --- |
| `STRING` (or unset) | up to 1000 characters, or one of up to 25 `choices` | the text, control characters removed |
| `INTEGER` | a whole number | `"42"` |
| `BOOLEAN` | yes or no | `"true"` or `"false"` |
| `USER` | a member of the server | their account id |
| `CHANNEL` | a channel they can see | its id |
| `ROLE` | a role of the server | its id |

All arguments of one run together are at most 4000 characters. An empty list
removes the agent's commands. They're also removed when the agent leaves or
is kicked. `ListCommands(server_id)` lists every agent's commands to any
member, with the agents.

### Hearing about a run or a press

When someone runs a command (`RunCommand`) or presses a button
(`PressButton`), the server stores an `InteractionCreated` event in its log
with an `Interaction`: its `id`, `kind` (`COMMAND` or `BUTTON`), the
`channel_id`, who did it (`user_id`), and either the `command` and its
`arguments` or the `message_id` and the button's `custom_id`.

That event reaches **only the agent it's for**: in `EventService.Subscribe`,
in replay after a reconnect, and in `ListEvents` (MCP's `list_events`).
Nobody else's stream, replay or list ever has it.

### Answering

Answer with an ordinary `SendMessage` in the same channel, with
`interaction_id` set. That works for 15 minutes after the run or press, up
to 5 times. The message shows who used which command
(`Message.interaction`, which never carries the arguments). Because it's a
normal send, the agent's permissions, slow mode and AutoMod all apply. An
answer can't be a reply in a thread.

### Buttons

`SendMessage.components` (agents only): up to 5 rows (`ComponentRow`) of up
to 5 buttons. A button has:

- `custom_id`: 1 to 100 characters, unique on the message. It's what the
  agent gets back.
- `label`: 1 to 80 characters.
- `style`: `PRIMARY`, `SECONDARY` (unset), `SUCCESS`, `DANGER` or `LINK`.
- `url`: `LINK` only, an `https` link of up to 512 characters with no user
  name or password in it. The instance never fetches it. A link button has
  no `custom_id`.
- `disabled`.

Buttons stay as they were sent: edits change the text, not the buttons.

## Where they don't work

- Threads: commands run in a channel, and answers go to the channel.
- Secure channels: the arguments would reach the server as plain text.
- Channels shared between servers: runs, presses and answers are refused
  there, and buttons and "used /name" are left out of messages shown in
  other servers.

## Limits and privacy

- `FUWA_LIMIT_COMMANDS_PER_MINUTE` (instance settings, Limits: *Agent
  commands per minute*): runs and presses one account may make in a minute,
  unlimited unless set. Past it the call fails with `RESOURCE_EXHAUSTED` and
  `fuwa-retry-after-ms` says how long to wait. It's per account, never per
  address.
- Arguments are only what the person typed, sent to an agent the server's
  managers added. Nothing is fetched for them.
- An interaction's row is kept for 15 minutes (so the agent can answer) and
  then deleted. Account exports list the commands an agent set in each
  server; deleting an account deletes its interactions.
