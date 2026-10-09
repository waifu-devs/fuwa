# The TypeScript SDK

`@waifu-devs/fuwa` is the official SDK for building agents (bots) and apps on
fuwa in JavaScript or TypeScript. It lives in [`sdk/`](../sdk) and runs in
Node 20 or newer and in browsers, as ES modules or CommonJS.

- **Typed clients** for every service in [`proto/fuwa/v1`](../proto/fuwa/v1),
  generated with the same buf setup as the web app, so they never drift from
  the protocol.
- **Agents**: sign in with the agent's token, follow every server it's in,
  answer commands and mentions, send, reply and edit, with reconnecting and
  catching up handled.
- **Voice**: join a voice channel, hear each person's sound labelled with
  who said it, know who's speaking, and talk, from Opus frames or an Ogg
  Opus file. No WebRTC: it's the instance's voice bridge for programs, so
  nothing but the instance is ever contacted.
- **Helpers**: paging through messages and events, uploading pictures,
  retries that respect slow mode, and errors you can tell apart with
  `instanceof`.

It has no dependencies beyond the protobuf and gRPC-Web runtime
(`@bufbuild/protobuf`, `@connectrpc/connect`, `@connectrpc/connect-web`),
reports nothing anywhere, and only ever talks to the instance you give it.

```sh
npm install @waifu-devs/fuwa
```

## An agent in a few lines

Make the agent in the app (Settings, Agents), copy its token, and add it to a
server (Server settings, Integrations). Then:

```ts
import { Agent } from "@waifu-devs/fuwa";

const agent = new Agent({ url: "https://fuwa.chat", token: process.env.FUWA_TOKEN! });

agent.command("ping", (ctx) => ctx.reply("pong"));
agent.command("echo", (ctx) => ctx.reply(ctx.rest), { description: "Says it back" });
agent.on("mention", (ctx) => ctx.reply("You called?"));

await agent.start();
```

A whole example, with dice, help and saving where it left off, is
[`sdk/examples/echo-agent.ts`](../sdk/examples/echo-agent.ts):

```sh
cd sdk && pnpm install && pnpm build
FUWA_URL=https://fuwa.chat FUWA_TOKEN=... pnpm example
```

### Agent tokens are secrets

Anyone with the token can read and post as the agent in every server it's
in. Keep it in the environment or a secret store, never in code or a
repository, and reset it (Settings, Agents, or `AgentService.ResetAgentToken`)
if it leaks: the old one stops working at once. The SDK sends it only to the
instance's address, in the `authorization` header, never logs it, and keeps
it out of error messages. It refuses plain `http://` to anything but this
computer unless you pass `allowInsecure: true`, since the token would cross
the network unencrypted.

### Commands and mentions

`agent.command(name, handler)` answers `/name args` (the prefix is
`prefix`, default `/`) and `@agent name args`. Names match in any case;
`aliases` adds more. The handler gets a `CommandContext`:

| | |
| --- | --- |
| `args`, `rest` | the words after the command, and all of it as written |
| `reply(text)` | answers in the channel, as a reply to the command |
| `send(text)` | posts in the channel |
| `author()` | the writer's profile (cached) |
| `message`, `serverId`, `channelId` | where it came from |

Text or a full request works for `reply` and `send`:
`ctx.reply({ content: "Look", embeds: [{ title: "A card" }] })`.

`agent.on("mention", ...)` runs for messages that mention the agent: by id
(`<@id>`, which the instance checks and lists in `message.mentionUserIds`)
or as `@username` (a whole word, in any case, as the apps decide it), and `agent.on("message", ...)` for
every message from someone else. `agent.commands` lists what's registered,
for a help command, and `unknownCommand` catches the rest.

By default an agent skips its own messages, other agents' (so two agents
can't answer each other forever) and webhook posts; turn the last two off
with `ignoreAgents: false` and `ignoreWebhooks: false`.

### Slash commands and buttons

Instances with slash commands (docs/commands.md) list an agent's commands
when members type "/", with typed options, and let it put buttons under its
messages. Using either sends the agent an interaction that only it sees:

```ts
import { CommandOptionType, InteractionKind } from "@waifu-devs/fuwa";

const commands = [
  { name: "roll", description: "Rolls dice", options: [{ name: "sides", description: "How many sides", type: CommandOptionType.INTEGER }] },
];
agent.on("ready", ({ servers }) => {
  for (const serverId of servers) void agent.setCommands(serverId, commands);
});
agent.on("serverAdded", (serverId) => void agent.setCommands(serverId, commands));

agent.on("interaction", async (ctx) => {
  if (ctx.kind === InteractionKind.COMMAND && ctx.command === "roll") {
    const sides = Number(ctx.options.sides ?? 6);
    await ctx.reply({
      content: `rolled ${1 + Math.floor(Math.random() * sides)}`,
      components: [{ buttons: [{ customId: "again", label: "Roll again" }] }],
    });
  } else if (ctx.kind === InteractionKind.BUTTON && ctx.customId === "again") {
    await ctx.reply("rolled again");
  }
});
```

- `agent.setCommands(serverId, commands)` replaces the agent's commands in
  a server (up to 50); commands are per server, so set them as the agent
  joins each one.
- `ctx.reply` answers with a message showing who used what (it sends
  `interactionId`), up to five times within 15 minutes. `components` on any
  message the agent sends adds rows of buttons; a `LINK` button opens its
  `url` and tells nobody.
- `ctx.options` has what was filled in, as text by option name. **The
  instance keeps typed arguments only while the interaction can be
  answered**: its event log never stores them, so an `interactionCreated`
  caught up later has none. Keep anything you need from them yourself.
  `interaction` handlers only get interactions that can still be
  answered; older ones come only as `interactionCreated`.
- `agent.api.commands` is the whole CommandService: `listCommands`, and
  `runCommand` and `pressButton` for apps that act for a person.

### Live tiles

An agent can keep a small card above a server's channel list up to date,
such as a match's scoreboard ([live-tiles.md](live-tiles.md)):

```ts
const tile = agent.liveTile(serverId, channelId, "final");
await tile.set({
  title: "Cup final",
  status: "67'",
  live: true,
  rows: [{ label: "Red Foxes", value: "2" }, { label: "Blue Owls", value: "1" }],
  progress: 0.74,
  action: "Watch",
});
await tile.end();
```

- `set` puts the tile up or changes it; call it as often as the score
  changes. People get at most about one update a second per tile (the
  instance's `live_tile_publish_ms`), always the latest.
- It's plain text in a fixed layout: a title (40 characters), a status
  (16), up to 4 rows of label (24) and value (8), progress 0 to 1 and a
  button label (12) that opens the channel. No links, markup or pictures;
  the agent's name always shows on it.
- A tile goes two hours after its last change unless `ttlSeconds` says
  otherwise, so a crashed program leaves nothing stale.
- It needs Send Messages in the channel, a text or announcement channel the
  server doesn't share, and the server showing tiles from apps; otherwise
  `set` fails with `PermissionDeniedError`, `InvalidArgumentError` or
  `FailedPreconditionError`. The server's AutoMod word and link rules apply.
- `agent.api.liveTiles.listLiveTiles({ serverId })` lists every app's tiles
  the agent can see.

### Reactions

```ts
agent.on("message", async (ctx) => {
  if (ctx.content.includes("good bot")) await ctx.react("💜");
});
agent.on("reactionUpdated", (payload) => {
  if (payload.added && payload.userId !== agent.me.id) console.log(payload.reaction?.emoji, payload.reaction?.count);
});
```

- `agent.react(message, emoji)` reacts, and `agent.react(message, emoji,
  false)` takes the agent's reaction off; both resolve with the emoji's
  reaction as it is now. An emoji is a standard one's characters ("👍") or
  one of the server's custom emoji (`<:name:id>`, or the `Emoji` itself).
- Reacting needs Add Reactions in the channel. A message holds at most the
  instance's `reactions_per_message` different emoji (unlimited unless set):
  past it, `react` fails with `RateLimitedError` (no `retryAfterMs`).
  Not in channels shared between servers yet (`FailedPreconditionError`).
- `agent.reactors(message, emoji)` is who reacted, the earliest first.
- Messages read through the API carry `reactions` (`me` for the agent);
  `reactionUpdated` and `reactionsCleared` events say what changes (`me` is
  never set in events: compare `userId`).

### Typed events

Every event kind has a handler named after its payload, with that payload's
type:

```ts
agent.on("memberJoined", (payload, event) => {
  console.log(payload.member?.user?.username, "joined", event.serverId);
});
agent.on("messageDeleted", (payload) => console.log("deleted", payload.messageId));
agent.on("event", (event) => {}); // every event, before the kind's handlers
```

Events in one channel are handled in order; channels run side by side.

### Staying connected

An agent follows all its servers over one `EventService.Subscribe` stream.
When it breaks (the instance restarts, the network drops, or nothing arrives
for 70 seconds, though the instance sends a heartbeat every 25), it emits
`disconnected` with the error and the wait, and reconnects with backoff from
each server's last sequence, so nothing is missed or handled twice. A token
that stops working ends it instead: `agent.closed` rejects with an
`UnauthenticatedError`.

To catch up on what happened while the program was stopped, save
`agent.cursors` (server id to the last sequence; bigints, so store them as
strings) and pass them back as `cursors`. Servers without one start with new
events only. The example saves them to a file every few seconds.

A server the agent is added to is followed the moment it's added
(`serverAdded`), on instances listing the `agent-streams` feature: the
stream asks for `follow_new_servers` and the instance announces each one
as `followed`. On older instances the agent lists its servers every
`serverRefreshMs` (30 seconds) instead. Being removed or the server being
deleted arrives as an event straight away (`serverRemoved`).
`EventFollower` takes `followNewServers` too, and yields a `followed`
update for each new server.

### When things go wrong

Errors from your handlers, a failed server refresh and a broken stream never
crash the agent: they go to `onError` if you pass one, else to your `error`
handlers, else to `console.error` as the error's name and message only.
Everything a call returns to you fails with a `FuwaError` subclass:

| Class | When |
| --- | --- |
| `UnauthenticatedError` | the token is missing, wrong, reset or revoked (`signedOut` is true) |
| `PermissionDeniedError` | the agent's roles don't allow it here |
| `NotFoundError` | the server, channel or message isn't there, or can't be seen |
| `InvalidArgumentError` | the request is wrong (empty, too long, a bad field) |
| `AlreadyExistsError` | something with that name is already there |
| `FailedPreconditionError` | not possible now, such as a feature the instance turned off |
| `RateLimitedError` | slow mode, a rate limit, or a cap like a full server; `retryAfterMs` is set when waiting helps |
| `UnavailableError` | the instance is unreachable, restarting or moving the server (`network` when there was no answer at all) |
| `TimeoutError`, `CanceledError`, `ServerError` | no answer in time, cancelled by you, or the instance failed |

Each has `code` (the gRPC status), `method`, `retryable` and the instance's
message. They're `ConnectError`s too, with the response's `metadata`.

### Retries and rate limits

Calls that fail in ways worth trying again are tried again, up to 4 times
with backoff from 400 ms to 20 seconds:

- slow mode and rate limits, after exactly as long as the instance asks (up
  to `maxRetryAfterMs`, a minute); caps like a full server fail at once;
- the instance answering that it's restarting or busy;
- network failures, but only for calls that read (`Get…`, `List…`): a write
  may have gone through, so sending it again could post twice.

Each try gets its own 30-second deadline (`timeoutMs`). Change any of it with
`retry: { retries, baseDelayMs, maxDelayMs, maxRetryAfterMs }`, or turn it off
with `retry: false`.

## Voice

An agent joins a voice channel with `agent.joinVoice(serverId, channelId)`.
It's in the channel as itself, with its roles (CONNECT to join, SPEAK to be
heard), and everyone sees it there with its AGENT badge. Under the hood it's
`CallService.ListenVoice` and `SpeakVoice` (docs/calls.md, "Agents, bots and
apps"): no WebRTC, no ICE, STUN or TURN, nothing but the instance.

```ts
const voice = await agent.joinVoice(serverId, channelId);

voice.on("speaking", (userId) => console.log(userId, "started talking"));
voice.on("silent", (userId) => console.log(userId, "stopped"));
voice.on("frame", (frame) => {
  // frame.userId said frame.opus: one Opus packet, 48 kHz, 20 ms.
});

await voice.play(readFileSync("hello.ogg")); // an Ogg Opus file, 20 ms frames
await voice.speak(opusFrames);               // or frames from any Opus encoder
await voice.setState({ selfMute: true });
await voice.leave();
```

- **Hearing**: each frame comes as an event and from `for await (const frame
  of voice)`. `speaking` and `silent` say when someone starts and stops
  sending sound (`silenceMs`, 300 ms by default; Opus's one-byte silence
  packets don't count). `voice.speaking` is who's talking now.
- **Talking**: `speak` takes Opus frames (48 kHz, 20 ms, mono or stereo) from
  a list or an async iterable. Each frame goes out as soon as it comes, and
  a source faster than real time is paced to about how fast it plays; it
  resolves with `{ interrupted, by, sentMs }` about when the last one is
  heard. Things said one after another wait their turn. `play` takes an Ogg
  Opus file (`ffmpeg -i in.wav -c:a libopus -frame_duration 20 out.ogg`) or
  a stream of one (a fetch `Response`, its body, any async iterable of
  bytes), which starts playing as soon as its first page arrives. Packets
  holding two or three 20 ms frames are split; other frame lengths are
  refused.
- **Files**: `readOggOpus`, `OggOpusReader` (pieces of any size as they
  arrive), `oggOpusPackets` (a stream) and `OggOpusWriter` (`flush` and
  `take` to send pages as they're written) read and write Ogg Opus in plain
  TypeScript.
- **Staying in**: when the connection drops or the instance restarts, it
  joins again with the same place (`reconnecting`, then `rejoined`), and
  `speak` waits for it. A stream that goes silent without closing counts
  as dropped too: instances send a keepalive every 15 s while nobody talks,
  and once one has come, `keepaliveTimeoutMs` (45 s) with nothing at all
  joins again. Being taken out by a moderator, kicked, losing
  CONNECT or joining from somewhere else ends it: `voice.closed` rejects with
  that error (FailedPrecondition). Stopping the agent leaves its channels.

### Conversations

For an agent people talk with, `utterance` is the unit to answer: what one
person says from when they start until they pause for `utteranceGapMs` (600
ms by default; 700 or so suits conversation). Its frames arrive while
they're still talking, so a speech-to-text service can start straight away.

```ts
const voice = await agent.joinVoice(serverId, channelId, { utteranceGapMs: 700 });

voice.on("utterance", async (utterance) => {
  // Stream it as it's spoken: Ogg Opus pieces every 100 ms...
  for await (const piece of utterance.ogg()) sendToSpeechService(piece);
  // ...or wait for the pause and take the whole file.
  await utterance.ended;
  const text = await transcribe(await utterance.toOgg());

  // Speak the answer as the speech service makes it.
  const reply = await fetch(ttsUrl, { method: "POST", body: ..., signal });
  const { interrupted } = await voice.play(reply, { interruptible: true });
});
```

- `for await (const u of voice.utterances())` is the same as the event.
  An utterance can be read any number of times, from its start, as frames
  (`for await`), Opus packets (`opus()`), Ogg Opus pieces (`ogg()`), a
  whole file once it ends (`toOgg()`) or PCM (`pcm(decoder)`). It ends at
  `maxUtteranceMs` (60 s) at the latest, and the next one begins.
- **Barge-in**: `interruptible: true` on `speak`, `play` or `speakPcm`
  stops as soon as someone starts talking over it (or pass a test of who may
  cut in), resolving with `interrupted: true` and who it was. Anything
  cut short tells the instance to drop what it still has queued, so the
  sound stops at once (older instances play it out, at most about 300 ms).
  `voice.stopSpeaking()` stops what's being said and everything waiting its
  turn; `voice.talking` says whether anything is.
- **Raw sound**: services that take or make 16-bit PCM instead of Opus need
  an Opus library of your choice (the SDK carries none), given through two
  small interfaces. `voice.speakPcm(pcm, { encoder, sampleRate, channels })`
  cuts PCM of any piece size into 20 ms frames and encodes them;
  `utterance.pcm(decoder)` decodes, with lost or unsent sound as silence so
  the timing stays true. `pcmFromBytes` and `pcmToBytes` convert to and
  from little-endian bytes.

```ts
import { OpusEncoder } from "@discordjs/opus";

const enc = new OpusEncoder(24_000, 1);
const encoder = { encode: (pcm: Int16Array) => enc.encode(Buffer.from(pcm.buffer, pcm.byteOffset, pcm.byteLength)) };
const res = await fetch(ttsUrl, { ... }); // 24 kHz 16-bit mono PCM
await voice.speakPcm(pcmFromBytes(res.body!), { encoder, sampleRate: 24_000, interruptible: true });
```

The SDK sends sound nowhere but the instance; an agent that passes people's
voices to a speech service is choosing to, and should say so where people
can see it (the example says which host it uses when it joins).
[`examples/voice-chat.ts`](../sdk/examples/voice-chat.ts) is a whole
conversation: each utterance goes to a speech-to-text service, a language
model answers, and each sentence is spoken as soon as it's written, with
barge-in. It works with any service shaped like OpenAI's API.

Voice channels only: calls in direct messages are end-to-end encrypted
between people's apps, and agents don't use direct messages. Cameras and
screen sharing aren't open to programs yet. [`examples/voice-agent.ts`](../sdk/examples/voice-agent.ts)
joins on `/join`, says back what each person said once they pause, and plays
a file on `/play`.

## The clients on their own

`createFuwa` gives typed clients for every service, for apps that aren't
agents (a dashboard, a moderation script, a person's own tools):

```ts
import { createFuwa, messages } from "@waifu-devs/fuwa";

const fuwa = createFuwa({ url: "https://fuwa.chat", token });
const { servers } = await fuwa.servers.listServers({});
const { version } = await fuwa.serverVersion();

for await (const { message, author } of messages(fuwa, { serverId, channelId })) {
  console.log(author?.username, message.content); // newest first; stop with break
}
```

`fuwa.node`, `auth`, `account`, `servers`, `channels`, `messages`, `events`,
`admin`, `media`, `roles`, `invites`, `join`, `dms`, `friends`, `secure`,
`automod`, `emojis`, `calls`, `webhooks`, `agents`, `sso`, `shared`,
`commands`, `gifs`, `presence` and `search` are the services;
every message type and enum is exported from the package too. `token` can be
a function, read on every call.

The other pieces:

- `messages(fuwa, { serverId, channelId, direction, from, pageSize })` walks
  a channel newest first (or `direction: "newer"` from a message), and
  `messagePages` gives whole pages with their authors.
- `listEvents(fuwa, serverId, { after })` reads a server's stored events.
- `EventFollower` is the reconnecting stream the agent uses: iterate it for
  `event`, `ready` and `disconnected` updates, and `setServers` to change what
  it follows.
- `uploadPicture(fuwa, { purpose, data, contentType })` (or `agent.upload`)
  uploads a picture the way the apps do (`MediaService.CreateUpload`, then a
  PUT of the bytes) and returns its `url`, for an avatar, an emoji or a server
  icon. The bytes only go to the instance's own address.
- `joinVoice(fuwa, { serverId, channelId })` is `agent.joinVoice` for any
  account.
- `parseCommand`, `mentions`, `roleMention` and `emoji` read and write the
  text conventions: commands, `@username`, `<@&role>` and `<:name:id>`.

## Versions

The SDK's version follows the server's: `@waifu-devs/fuwa@0.4.2` is built
from the same protocol as fuwa v0.4.2. `fuwa.serverVersion()` says what an
instance runs (`NodeService.GetNode`), so an app can tell when a feature it
wants needs a newer instance. Instances that don't know a call answer it with
`ServerError` (`code` Unimplemented).

## Working on the SDK

```sh
cd sdk
pnpm install
pnpm generate        # regenerate src/gen after changing proto/
pnpm build           # dist/esm and dist/cjs, with types
pnpm typecheck
pnpm test            # unit tests (against dist/)
cargo build -p fuwa-server --bin fuwa
FUWA_BIN=../target/debug/fuwa pnpm test:instance   # against a real instance
```

The instance tests start the binary on a free port with its data in a
temporary folder and calls on: a person makes a server and an agent, and the agent answers
commands, catches up after being stopped, reconnects across a restart of the
instance, notices a new server, waits out slow mode and uploads a picture.
CI runs all of it when `sdk/` or `proto/` change.

Releases: pushing a `v*` tag (the server's release tag) runs
`.github/workflows/sdk-release.yml`, which publishes the SDK to npm at the
tag's version with provenance. It uses npm's trusted publishing, so there's no
npm token: the package's trusted publisher on npmjs.com names the `waifu-devs/fuwa`
repository, the `sdk-release.yml` workflow and the `npm` environment (limited to
`v*` tags), and the job signs in with GitHub's OIDC token.
