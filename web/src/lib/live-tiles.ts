import type { Channel, LiveTile, Message, VoiceState } from "@/gen/fuwa/v1/types_pb";
import type { Timestamp } from "@bufbuild/protobuf/wkt";

/**
 * Live tiles: small cards at the top of a server's channel list for things
 * happening there right now (people talking in a voice room, a poll about to
 * close, a thread you follow moving, a busy shared channel, an app's
 * scoreboard). Which kinds show is the server's setting (Server.liveTiles);
 * each person can still turn them off here (live-tiles-store.ts).
 *
 * Every tile but an app's is built from what this app already holds and
 * already shows somewhere else, so a tile never tells anyone more than the
 * sidebar or the channel would. App tiles come from the server
 * (LiveTileService), only for channels you can see (docs/live-tiles.md).
 */

// ChannelType's and LiveTileKind's values (types_pb), so node's test runner reads this file as is.
const SECURE = 6;
const KIND_NUMBERS: Record<number, TileKind> = { 1: "voice", 2: "poll", 3: "thread", 4: "shared", 5: "app" };
const WEBHOOK = 2;

const ms = (ts: Timestamp | undefined) => (ts ? Number(ts.seconds) * 1000 + Math.floor(ts.nanos / 1e6) : 0);

export type TileKind = "voice" | "poll" | "thread" | "shared" | "app";

type Base = {
  /** Stable while the activity lasts, and new when it starts again: hiding a tile hides this one only. */
  id: string;
  channelId: string;
};

export type VoiceTile = Base & { kind: "voice"; channelName: string; userIds: string[]; since: number; video: boolean; screen: boolean };
export type PollTile = Base & { kind: "poll"; channelName: string; messageId: string; question: string; voters: number; endsAt: number | null };
export type ThreadTile = Base & { kind: "thread"; threadId: string; title: string; replies: number; unread: number; userIds: string[] };
export type SharedTile = Base & { kind: "shared"; channelName: string; unread: number; servers: number };
/**
 * A tile an app (an agent or a webhook) keeps up to date in a server, such as
 * a match's scoreboard. Apps fill a fixed template (the server holds them to
 * its limits, and `fitApp` again here) and never send markup, scripts,
 * styles or outside links; the app's name always shows on it.
 */
export type AppTile = Base & {
  kind: "app";
  /** The app's name, shown with its badge so nobody mistakes the tile for fuwa's own or a person's. */
  app: string;
  /** Its picture, through the instance like every avatar. */
  avatarUrl: string;
  webhook: boolean;
  title: string;
  /** A short state, such as "67'" or "Half time". */
  status: string;
  /** Going on right now: the tile gets the live dot. */
  live: boolean;
  /** Up to four label and value pairs: teams and scores, players and points. */
  rows: { label: string; value: string }[];
  /** How far along, 0 to 1, for the bar along the bottom; null for none. */
  progress: number | null;
  /** The button's words, empty for "Open"; it always opens the tile's channel. */
  action: string;
  /** When it goes unless the app changes it again. */
  expiresAt: number;
};

export type Tile = VoiceTile | PollTile | ThreadTile | SharedTile | AppTile;

/** The template's limits: what an app sends is cut to these before anyone sees it. */
export const CUSTOM_LIMITS = { title: 40, status: 16, rows: 4, label: 24, value: 8, action: 12 } as const;

// Control, zero-width and direction-override characters: an app could use them to make its name or a row read as something else.
const HIDDEN_CHARS = /[\p{Cc}\u200B-\u200F\u202A-\u202E\u2066-\u2069\uFEFF]/gu;

const cut = (text: string, max: number) => {
  const flat = text.replace(/\s+/g, " ").replace(HIDDEN_CHARS, "").trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
};

/** An app's tile as it may show: text on one line, without hidden characters, cut to the limits, at most four rows, progress kept between 0 and 1. */
export function fitApp(tile: AppTile): AppTile {
  return {
    ...tile,
    app: cut(tile.app, CUSTOM_LIMITS.title),
    title: cut(tile.title, CUSTOM_LIMITS.title),
    status: cut(tile.status, CUSTOM_LIMITS.status),
    rows: tile.rows.slice(0, CUSTOM_LIMITS.rows).map((r) => ({ label: cut(r.label, CUSTOM_LIMITS.label), value: cut(r.value, CUSTOM_LIMITS.value) })),
    progress: tile.progress === null || !Number.isFinite(tile.progress) ? null : Math.min(1, Math.max(0, tile.progress)),
    action: cut(tile.action, CUSTOM_LIMITS.action),
  };
}

/** At most this many tiles show in a server; the rest wait their turn. */
export const MAX_TILES = 3;
/** A voice room is a tile from this many people: one person alone isn't a gathering to point at. */
export const VOICE_MIN = 2;
/** A poll is a tile while it closes within this long (or runs with no end and is new). */
export const POLL_SOON_MS = 6 * 60 * 60_000;
/** A shared channel is busy from this many unread messages. */
export const SHARED_MIN = 10;

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
  /** Apps' tiles, as the server listed and sent them. */
  apps: LiveTile[];
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

  // Apps' tiles that haven't run out. Hiding one hides it for as long as its app keeps it.
  for (const t of s.apps) {
    const expiresAt = ms(t.expiresAt);
    if (expiresAt <= now || !shown(t.channelId)) continue;
    const c = t.content;
    tiles.push(
      fitApp({
        kind: "app",
        id: `app:${t.sourceId}:${t.channelId}:${t.id}`,
        channelId: t.channelId,
        app: t.sourceName,
        avatarUrl: t.sourceAvatarUrl,
        webhook: t.sourceKind === WEBHOOK,
        title: c?.title ?? "",
        status: c?.status ?? "",
        live: !!c?.live,
        rows: (c?.rows ?? []).map((r) => ({ label: r.label, value: r.value })),
        progress: c?.progress ?? null,
        action: c?.action ?? "",
        expiresAt,
      }),
    );
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
    case "poll":
      return tile.endsAt === null ? 30 : 50 + 25 * (1 - Math.min(tile.endsAt - now, POLL_SOON_MS) / POLL_SOON_MS);
    case "voice":
      return 40 + Math.min(tile.userIds.length, 20);
    case "thread":
      return 25 + Math.min(tile.unread, 10);
    case "shared":
      return 20 + Math.min(tile.unread / 10, 10);
    case "app":
      // An app can't buy its way to the top: a live tile sits with busy rooms, a quiet one below.
      return tile.live ? 55 : 35;
  }
}

/** From this many members voice room tiles start off, unless the server's managers chose (the server's own `BIG_SERVER`). */
export const BIG_SERVER = 500;

export const TILE_KINDS: readonly TileKind[] = ["voice", "poll", "thread", "shared", "app"];

/** The kinds a server shows, from its setting (LiveTileKind numbers); none when the instance doesn't say. */
export function serverKinds(kinds: readonly number[] | undefined): Set<TileKind> {
  return new Set((kinds ?? []).flatMap((k) => (KIND_NUMBERS[k] ? [KIND_NUMBERS[k]] : [])));
}

/** A server's kinds as its setting takes them back: one number each. */
export function kindNumbers(kinds: ReadonlySet<TileKind>): number[] {
  return Object.entries(KIND_NUMBERS)
    .filter(([, kind]) => kinds.has(kind))
    .map(([n]) => Number(n));
}

/** The tiles to show: hidden ones and kinds the server turned off out, the most urgent first, at most `max`. Ties keep their order. */
export function pickTiles(tiles: Tile[], hidden: ReadonlySet<string>, now: number, kinds: ReadonlySet<TileKind> = new Set(TILE_KINDS), max = MAX_TILES): Tile[] {
  return tiles
    .filter((t) => kinds.has(t.kind) && !hidden.has(t.id))
    .map((t, n) => ({ t, n, u: urgency(t, now) }))
    .sort((a, b) => b.u - a.u || a.n - b.n)
    .slice(0, max)
    .map(({ t }) => t);
}
