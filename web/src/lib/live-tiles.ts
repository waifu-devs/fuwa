import type { Channel, Message, VoiceState } from "@/gen/fuwa/v1/types_pb";
import type { Timestamp } from "@bufbuild/protobuf/wkt";

/**
 * Live tiles: small cards at the top of a server's channel list for things
 * happening there right now (people talking in a voice room, a poll about to
 * close, a thread you follow moving, a busy shared channel, an event about to
 * start). A draft, off unless this browser turns it on (live-tiles-store.ts).
 *
 * Every tile is built from what this app already holds and already shows
 * somewhere else, so a tile never tells anyone more than the sidebar or the
 * channel would. Scheduled events don't exist on the server yet: those tiles
 * come from `sampleTiles` and say they're samples.
 */

// ChannelType's values (types_pb), so node's test runner reads this file as is.
const TEXT = 1;
const SECURE = 6;

const ms = (ts: Timestamp | undefined) => (ts ? Number(ts.seconds) * 1000 + Math.floor(ts.nanos / 1e6) : 0);

export type TileKind = "event" | "voice" | "poll" | "thread" | "shared";

type Base = {
  /** Stable while the activity lasts, and new when it starts again: hiding a tile hides this one only. */
  id: string;
  channelId: string;
  /** Made up on this device to show the idea, never from the server. */
  sample?: boolean;
};

export type VoiceTile = Base & { kind: "voice"; channelName: string; userIds: string[]; since: number; video: boolean; screen: boolean };
export type PollTile = Base & { kind: "poll"; channelName: string; messageId: string; question: string; voters: number; endsAt: number | null };
export type ThreadTile = Base & { kind: "thread"; threadId: string; title: string; replies: number; unread: number; userIds: string[] };
export type SharedTile = Base & { kind: "shared"; channelName: string; unread: number; servers: number };
export type EventTile = Base & { kind: "event"; title: string; startsAt: number; going: number };

export type Tile = VoiceTile | PollTile | ThreadTile | SharedTile | EventTile;

/** At most this many tiles show in a server; the rest wait their turn. */
export const MAX_TILES = 3;
/** A voice room is a tile from this many people: one person alone isn't a gathering to point at. */
export const VOICE_MIN = 2;
/** A poll is a tile while it closes within this long (or runs with no end and is new). */
export const POLL_SOON_MS = 6 * 60 * 60_000;
/** A shared channel is busy from this many unread messages. */
export const SHARED_MIN = 10;
/** An event is a tile from this long before it starts until a while after. */
export const EVENT_AHEAD_MS = 60 * 60_000;
export const EVENT_AFTER_MS = 30 * 60_000;

export type Sources = {
  channels: Channel[];
  voice: VoiceState[];
  /** Loaded messages, per channel. */
  messages: Record<string, { items: Message[] } | undefined>;
  threadParents: Record<string, Message>;
  followed: Record<string, true> | undefined;
  threadUnread: Record<string, number>;
  unread: Record<string, number>;
  /** The voice channel you're in, if any: no tile asks you to join where you are. */
  inChannel: string | null;
  /** Channels whose notifications you muted: they don't get tiles either. */
  muted: (channelId: string) => boolean;
};

const firstLine = (text: string, max = 80) => {
  const line = text.split("\n").find((l) => l.trim())?.trim() ?? "";
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
};

/**
 * The tiles a server has now, from what this app already knows. Only channels
 * in `channels` (the ones the sidebar lists, which the server already limited
 * to what you can see) count, and secure channels never do: their text is
 * end-to-end encrypted and stays inside the channel.
 */
export function collectTiles(s: Sources, now: number): Tile[] {
  const tiles: Tile[] = [];
  const byId = new Map(s.channels.map((c) => [c.id, c]));
  const shown = (id: string) => {
    const c = byId.get(id);
    return !!c && c.type !== SECURE && !s.muted(id);
  };

  // Voice rooms with people in them.
  const rooms = new Map<string, VoiceState[]>();
  for (const v of s.voice) if (v.channelId) rooms.set(v.channelId, [...(rooms.get(v.channelId) ?? []), v]);
  for (const [channelId, states] of rooms) {
    if (states.length < VOICE_MIN || channelId === s.inChannel || !shown(channelId)) continue;
    const since = Math.min(...states.map((v) => ms(v.joinedAt)));
    tiles.push({
      kind: "voice",
      id: `voice:${channelId}:${since}`,
      channelId,
      channelName: byId.get(channelId)!.name,
      userIds: states.map((v) => v.userId),
      since,
      video: states.some((v) => v.selfVideo),
      screen: states.some((v) => v.selfStream),
    });
  }

  // Open polls you haven't answered, in channels this app loaded.
  for (const [channelId, loaded] of Object.entries(s.messages)) {
    if (!loaded || !shown(channelId)) continue;
    for (const m of loaded.items) {
      const poll = m.poll;
      if (!poll || poll.endedAt || poll.myAnswerIds.length) continue;
      const endsAt = poll.endsAt ? ms(poll.endsAt) : null;
      if (endsAt !== null && (endsAt <= now || endsAt - now > POLL_SOON_MS)) continue;
      if (endsAt === null && now - ms(m.createdAt) > POLL_SOON_MS) continue;
      tiles.push({
        kind: "poll",
        id: `poll:${m.id}`,
        channelId,
        channelName: byId.get(channelId)!.name,
        messageId: m.id,
        question: firstLine(poll.question),
        voters: Number(poll.voters),
        endsAt,
      });
    }
  }

  // Threads you follow with replies you haven't read.
  for (const [threadId, unread] of Object.entries(s.threadUnread)) {
    const parent = s.threadParents[threadId] ?? Object.values(s.messages).find((l) => l?.items.some((m) => m.id === threadId))?.items.find((m) => m.id === threadId);
    if (!unread || !parent || !s.followed?.[threadId] || !shown(parent.channelId)) continue;
    tiles.push({
      kind: "thread",
      id: `thread:${threadId}`,
      channelId: parent.channelId,
      threadId,
      title: firstLine(parent.content) || byId.get(parent.channelId)!.name,
      replies: parent.thread?.replyCount ?? 0,
      unread,
      userIds: parent.thread?.participantIds ?? [],
    });
  }

  // Shared channels with a lot going on since you last looked.
  for (const c of s.channels) {
    const unread = s.unread[c.id] ?? 0;
    if (!c.shared || unread < SHARED_MIN || !shown(c.id)) continue;
    tiles.push({ kind: "shared", id: `shared:${c.id}`, channelId: c.id, channelName: c.name, unread, servers: 1 + c.shared.guests.length });
  }

  return tiles;
}

/**
 * How much a tile matters now, higher first. Things with a clock (an event
 * starting, a poll closing) rise as the moment comes; rooms rise with the
 * people in them; the rest sit below.
 */
export function urgency(tile: Tile, now: number): number {
  switch (tile.kind) {
    case "event": {
      const until = tile.startsAt - now;
      return until <= 0 ? 90 : 80 + 10 * (1 - Math.min(until, EVENT_AHEAD_MS) / EVENT_AHEAD_MS);
    }
    case "poll":
      return tile.endsAt === null ? 30 : 50 + 25 * (1 - Math.min(tile.endsAt - now, POLL_SOON_MS) / POLL_SOON_MS);
    case "voice":
      return 40 + Math.min(tile.userIds.length, 20);
    case "thread":
      return 25 + Math.min(tile.unread, 10);
    case "shared":
      return 20 + Math.min(tile.unread / 10, 10);
  }
}

/** Whether an event tile still belongs on screen. */
export const eventLive = (startsAt: number, now: number) => startsAt - now <= EVENT_AHEAD_MS && now - startsAt <= EVENT_AFTER_MS;

/** The tiles to show: hidden ones out, the most urgent first, at most `max`. Ties keep their order. */
export function pickTiles(tiles: Tile[], hidden: ReadonlySet<string>, now: number, max = MAX_TILES): Tile[] {
  return tiles
    .filter((t) => !hidden.has(t.id) && (t.kind !== "event" || eventLive(t.startsAt, now)))
    .map((t, n) => ({ t, n, u: urgency(t, now) }))
    .sort((a, b) => b.u - a.u || a.n - b.n)
    .slice(0, max)
    .map(({ t }) => t);
}

// ───────────────────────── Samples ─────────────────────────

/**
 * Made-up tiles for what the server can't tell yet (scheduled events, and a
 * busy shared channel when there's none), so the idea can be seen. Only in
 * the "demo" mode, and every one is marked as a sample. Times hang off
 * `anchor`, the moment the demo started, so they count down like real ones.
 */
export function sampleTiles(channels: Channel[], anchor: number, eventTitle: string): Tile[] {
  const text = channels.find((c) => c.type === TEXT);
  if (!text) return [];
  return [
    { kind: "event", id: `sample:event:${text.id}`, sample: true, channelId: text.id, title: eventTitle, startsAt: anchor + 12 * 60_000, going: 14 },
    { kind: "shared", id: `sample:shared:${text.id}`, sample: true, channelId: text.id, channelName: text.name, unread: 42, servers: 3 },
  ];
}
