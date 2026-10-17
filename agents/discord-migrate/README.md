# discord-migrate

A Discord bot that copies a Discord server into a fuwa server, so a community
can move over without rebuilding everything by hand. It's two bots in one
process: a Discord bot that reads the server, and a fuwa agent that builds it
again, built on the TypeScript SDK ([docs/sdk.md](../../docs/sdk.md)) and
discord.js.

## How a server moves

1. A Discord administrator adds the bot to their server and runs
   **`/migrate start`**. It answers, only to them, with a code like
   `K7QD-M2XP` (it works once, for 30 minutes) and what to do next.
2. On fuwa, they make a server for the community (or open one they manage),
   add the agent (Server settings > Integrations) and give it a role with
   Administrator at the top of the role list.
3. They run **`/import K7QD-M2XP`** in any channel there. Only the server's
   owner or people with Administrator or Manage Server can.

The bot then copies the server, keeping a progress message up to date in the
Discord channel where `/migrate start` ran and in the fuwa channel where
`/import` ran, and posts a summary in both when it's done: what came over,
the invite for members, and what didn't come over and why. `/migrate cancel`
stops it.

`/migrate start` takes two options: `messages`, how many of each text
channel's latest messages to copy (`MESSAGES_PER_CHANNEL` unless set, 0 for
none), and `announce`, a Discord channel to post the fuwa invite in when it's
done.

## What comes over

| Discord | fuwa |
| --- | --- |
| Name, description, icon, banner | The server's |
| Roles, with colors, hoisting, mentionability and permissions, in order | Roles (bots' own roles stay behind) |
| @everyone's permissions | @everyone's |
| Categories, text, announcement and voice channels (stage channels as voice), in order, with topics and slow mode | The same kinds |
| Channel permission overwrites for roles | Overwrites, for what fuwa lets channels change |
| Forum and media channels | Text channels: each post opens with its first message, titled, and the rest is its thread |
| Custom emoji, and PNG, APNG and GIF stickers | Custom emoji (names made to fit fuwa's rules) |
| Soundboard sounds, with their emoji and volume | Soundboard sounds, turned into Ogg Opus with ffmpeg ([docs/soundboard.md](../../docs/soundboard.md)) |
| Members | One invite that never runs out |
| Each text channel's latest messages | Posted through a webhook per channel under each author's name and picture, with replies quoted, mentions of roles, channels and emoji pointing at the copies, bots' embeds, reactions listed under each message, a divider at each new day (a timestamp everyone reads in their own time zone), files sent right after by the agent, and pins pinned again |
| Threads started from those messages | Threads under the copies. fuwa's webhooks can't post in threads, so the agent posts each reply, starting with its author's name and when they said it |

Never copied: members' roles (people join with the invite), private threads,
Lottie stickers, and member overwrites. Reactions are listed as text, since
the agent can't react as the people who did. Old messages are posted now, so
they carry the time they were copied (fuwa has no way to post into the past);
the day dividers, and the line each thread reply starts with, keep when they
were said. Each thread, and each forum's posts, take up to `messages` too.

Sounds need an instance with a soundboard: on one without, they're listed as
not copied and roles leave out the soundboard's permissions. Discord keeps
sounds as MP3 or Ogg, and fuwa takes Ogg Opus, so the bot needs `ffmpeg` (the
image has it) for any that aren't. A webhook posts at most 30 messages a minute, so
history takes about two seconds a message per channel, with `CHANNELS_AT_ONCE`
channels at a time: 1,000 messages in each of 8 channels takes a bit over an
hour.

Every step is written down as it's done (`DATA_DIR`, one file per Discord
server and fuwa server), so a run that stops (cancelled, a restart, the
instance away) picks up where it left off: run `/migrate start` again and use
the new code in the same fuwa server. Nothing is copied twice.

When fuwa turns one thing down for good (AutoMod, a full server, a name it
won't take), that thing is skipped and listed in the summary; anything else
stops the run, to be picked up later.

## Setting up the Discord bot

1. Make an application in the [Discord Developer Portal](https://discord.com/developers/applications)
   and a bot for it; its token is `DISCORD_TOKEN`.
2. Under Bot, turn on the **Message Content** intent, or the bot can copy
   servers' structure but not their messages. A bot in 100 servers or more
   needs Discord to approve the intent.
3. When it starts, the bot logs a link that adds it to a server with what it
   needs: View Channels and Read Message History to read, Send Messages and
   Embed Links for its progress. A channel the bot can't read still comes
   over, without its messages.

## Settings

| Variable | |
| --- | --- |
| `DISCORD_TOKEN` | the Discord bot's token. A secret |
| `FUWA_URL` | the fuwa instance, `https://fuwa.chat` by default |
| `FUWA_TOKEN` | the fuwa agent's token (Settings > Agents). A secret |
| `DATA_DIR` | where each migration's progress is kept, `./data` by default (`/data` in the image: put a volume there) |
| `MESSAGES_PER_CHANNEL` | how many of each channel's latest messages `/migrate start` copies unless told, `1000` by default |
| `MAX_MESSAGES` | the most `/migrate start` may be asked for per channel, `50000` by default |
| `CHANNELS_AT_ONCE` | how many channels' histories are copied at once, `4` by default |
| `MAX_FILE_MB` | files bigger than this are named in the message instead of copied, `25` by default |

## Running it

```sh
pnpm install
DISCORD_TOKEN=... FUWA_TOKEN=... pnpm start   # Node 24 runs the TypeScript as it is
pnpm typecheck && pnpm test
FUWA_BIN=../../target/debug/fuwa pnpm test:instance   # the fuwa side against a real instance
```

`src/migrate.ts` is the migration, over the two sides in `src/model.ts`:
`src/discord.ts` reads Discord and `src/fuwa.ts` writes fuwa, so the tests run
it against fakes (`test/migrate.test.ts`) and a real instance
(`test/instance.test.ts`). `src/convert.ts` turns Discord's permissions,
channel order, names and message text into fuwa's.
