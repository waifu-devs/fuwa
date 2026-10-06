# Live tiles

Small cards at the top of a server's channel list about what's happening
there right now: people talking in a voice channel, a poll about to close,
new replies in a thread you follow, a busy shared channel, and tiles apps
keep up to date (a match's scoreboard, a raid timer). Instances that have
them list `live-tiles` in `Node.versions.features`.

The server keeps two things: which kinds each server shows
(`Server.live_tiles`), and the tiles apps set (`fuwa.v1.LiveTileService`,
`proto/fuwa/v1/live_tile.proto`). Every other tile is worked out by the app
from what it already holds and already shows somewhere else.

## What people see

- At most three tiles, the most pressing first: a poll closing soon, then a
  live app tile or a busy voice room, then quieter ones. Each opens its
  channel (a voice tile joins the call, a thread tile opens the thread).
- Nothing while you're on Do not disturb or have the server muted, and no
  tile for a channel you muted, a secure channel, or a channel you can't see.
- Each person can hide one tile, turn tiles off for one server, or turn them
  off everywhere. These choices stay on the device.
- Never your own voice room, and never online status or activities: a voice
  tile shows only who's in the call, as the channel list does.

## What a server shows

Whoever has Manage Server picks the kinds in the server menu ("Tiles for
everyone here"): voice rooms, polls, followed threads, shared channels and
tiles from apps. `UpdateServer` with `live_tiles` saves the choice (an
audit entry "live_tiles"); `customized: false` goes back to the default.

The default shows every kind except voice rooms in servers of 500 members or
more (`servers::BIG_SERVER`), so a crowd isn't pointed at a few people
talking. The server resolves it in `Server.live_tiles.kinds`; since a
server's size changes without a `ServerUpdated`, apps work the default out
the same way from `member_count` while `customized` is false.

## Tiles from apps

An agent sets a tile with `SetLiveTile`; a webhook with an HTTP POST to its
address plus `/tile`:

```
POST <instance>/webhooks/<server id>/<webhook id>/<token>/tile
{"id": "final", "title": "Cup final", "status": "67'", "live": true,
 "rows": [{"label": "Red Foxes", "value": "2"}], "progress": 0.74,
 "action": "Watch", "ttl_seconds": 7200}
```

and ends it with `DELETE` to the same address and `?id=final`. Both answer
204; a wrong token or webhook is 404, as for messages.

- The app's id for a tile (`tile_id`) is 1 to 64 letters, digits, `-` and
  `_`; setting it again changes it.
- Plain text in a fixed layout: a title (1 to 40 characters), a status (16),
  live or not, up to 4 rows of label (24) and value (8), progress 0 to 1 and
  a button label (12). The server trims every field, folds runs of spaces and
  takes out control, zero-width and direction characters; apps cut the
  same way again. No markup, links, colors or pictures: the button only opens
  the tile's channel, and the app's name (the agent's, or the webhook's) and
  badge always show on it.
- Only in the server's own text and announcement channels: never secure
  ones, nor channels shared with other servers. Only while the server shows
  tiles from apps.
- Agents need Send Messages in the channel; people can't set tiles. A
  webhook's tiles go in its channel. An agent that loses Send Messages there
  stops showing its tiles in `ListLiveTiles`.
- The server's AutoMod word, link and ping rules read the tile's text and
  can refuse it (an error starting "AutoMod: "). A tile can change every
  second, so it posts no alerts, times nobody out, and the Smart filter's
  provider isn't asked. Agents with Manage Server aren't caught, as with
  messages.
- A tile goes `ttl_seconds` after its last change (2 hours unless set), so a
  crashed program leaves nothing stale. It also goes when its channel is
  deleted, its agent leaves the server, or its webhook is deleted or moved.
- `EndLiveTile` ends the caller's own tile; with Manage Messages in the
  channel, anyone's (`source_id`), with an audit entry.

### How changes reach people

Tiles are kept in the server's file (`live_tiles`), but changing one isn't
an event in the log: `LiveTileUpdated` and `LiveTileEnded` go out with
sequence 0, like voice states, only to people who can see the channel. After
a reconnect apps call `ListLiveTiles` again. A tile changed faster than the
instance's `live_tile_publish_ms` (1000 unless set; a protective default) is
stored each time but sent at most once per interval, with its latest
content, so a busy scoreboard can't flood everyone. `LiveTileEnded` isn't
sent when the channel goes or the agent leaves: apps drop those tiles on
`ChannelDeleted` and `MemberLeft`, and any tile past its `expires_at`.

### Limits

Instance admins set these in the instance settings (Limits):

- `live_tiles_per_channel` (`FUWA_LIMIT_LIVE_TILES_PER_CHANNEL`): app tiles
  one channel holds at once; unlimited unless set.
- `live_tile_updates_per_minute` (`FUWA_LIVE_TILE_UPDATES_PER_MINUTE`): sets
  and ends per agent or webhook per server a minute; 120 unless set, a
  protective default (`unlimited` in the variable, or clearing it in the
  settings, removes it).

MCP agents get `set_live_tile`, `end_live_tile` and `list_live_tiles`
([mcp.md](mcp.md)); the SDK has `agent.liveTile` ([sdk.md](sdk.md)).

## Later

Scheduled events (an "Events" kind), the per-person switch on the account
so it follows you across devices, tiles in shared channels, pictures on app
tiles, and the desktop app.
