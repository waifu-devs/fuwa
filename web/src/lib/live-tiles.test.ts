import assert from "node:assert/strict";
import { test } from "node:test";
import type { Channel, Message, VoiceState } from "@/gen/fuwa/v1/types_pb";
import { collectTiles, enabledKinds, fitCustom, pickTiles, type Sources, type Tile } from "./live-tiles.ts";

const NOW = 1_800_000_000_000;
const ts = (ms: number) => ({ seconds: BigInt(Math.floor(ms / 1000)), nanos: (ms % 1000) * 1e6 }) as Message["createdAt"];
const channel = (id: string, type = 1, extra: Partial<Channel> = {}) => ({ id, name: id, type, ...extra }) as Channel;
const voice = (userId: string, channelId: string, joined = NOW - 60_000) => ({ userId, channelId, joinedAt: ts(joined) }) as VoiceState;
const poll = (id: string, extra: Partial<NonNullable<Message["poll"]>> = {}) =>
  ({ id, channelId: "general", createdAt: ts(NOW - 60_000), content: "", poll: { question: "Pizza?", voters: 3n, myAnswerIds: [], ...extra } }) as unknown as Message;

const sources = (extra: Partial<Sources> = {}): Sources => ({
  channels: [channel("general"), channel("lounge", 2), channel("vault", 6)],
  voice: [],
  messages: {},
  threadParents: {},
  followed: {},
  threadUnread: {},
  unread: {},
  inChannel: null,
  muted: () => false,
  ...extra,
});

const kinds = (tiles: Tile[]) => tiles.map((t) => `${t.kind}:${t.channelId}`);

test("a voice room is a tile once two people are in it, and never the room you're in", () => {
  assert.deepEqual(kinds(collectTiles(sources({ voice: [voice("a", "lounge")] }), NOW)), []);
  const two = [voice("a", "lounge", NOW - 90_000), voice("b", "lounge", NOW - 30_000)];
  const tiles = collectTiles(sources({ voice: two }), NOW);
  assert.deepEqual(kinds(tiles), ["voice:lounge"]);
  assert.equal(tiles[0]!.kind === "voice" && tiles[0]!.since, NOW - 90_000);
  assert.deepEqual(collectTiles(sources({ voice: two, inChannel: "lounge" }), NOW), []);
});

test("no tile tells of a channel the sidebar doesn't list, a secure one or a muted one", () => {
  const hidden = [voice("a", "staff"), voice("b", "staff")];
  assert.deepEqual(collectTiles(sources({ voice: hidden }), NOW), []);
  const secret = [voice("a", "vault"), voice("b", "vault")];
  assert.deepEqual(collectTiles(sources({ voice: secret }), NOW), []);
  const muted = [voice("a", "lounge"), voice("b", "lounge")];
  assert.deepEqual(collectTiles(sources({ voice: muted, muted: (id) => id === "lounge" }), NOW), []);
  const thread = { channelId: "vault", content: "secret plans", thread: { replyCount: 4, participantIds: [] } } as unknown as Message;
  assert.deepEqual(collectTiles(sources({ threadParents: { t1: thread }, followed: { t1: true }, threadUnread: { t1: 2 } }), NOW), []);
});

test("a poll is a tile while it's open, closing soon, and you haven't voted", () => {
  const items = [
    poll("soon", { endsAt: ts(NOW + 30 * 60_000) }),
    poll("voted", { endsAt: ts(NOW + 30 * 60_000), myAnswerIds: [1] }),
    poll("ended", { endedAt: ts(NOW - 1000) }),
    poll("past", { endsAt: ts(NOW - 1000) }),
    poll("far", { endsAt: ts(NOW + 7 * 24 * 3600_000) }),
  ];
  const tiles = collectTiles(sources({ messages: { general: { items } } }), NOW);
  assert.deepEqual(tiles.map((t) => t.id), ["poll:soon"]);
});

test("a thread is a tile only when you follow it and it has replies you haven't read", () => {
  const parent = { channelId: "general", content: "\n  Release notes for v2\nmore", thread: { replyCount: 9, participantIds: ["a"] } } as unknown as Message;
  const base = { threadParents: { t1: parent }, threadUnread: { t1: 3 } };
  assert.deepEqual(collectTiles(sources({ ...base, followed: {} }), NOW), []);
  const [tile] = collectTiles(sources({ ...base, followed: { t1: true } }), NOW);
  assert.equal(tile?.kind === "thread" && tile.title, "Release notes for v2");
});

test("a shared channel is a tile once enough has happened in it", () => {
  const shared = channel("bridge", 1, { shared: { guests: [{}, {}] } as unknown as Channel["shared"] });
  const channels = [shared];
  assert.deepEqual(collectTiles(sources({ channels, unread: { bridge: 9 } }), NOW), []);
  const [tile] = collectTiles(sources({ channels, unread: { bridge: 10 } }), NOW);
  assert.equal(tile?.kind === "shared" && tile.servers, 3);
});

test("the most urgent tiles show, up to the cap, without the hidden ones", () => {
  const tiles: Tile[] = [
    { kind: "thread", id: "thread", channelId: "c", threadId: "t", title: "", replies: 1, unread: 1, userIds: [] },
    { kind: "voice", id: "voice", channelId: "c", channelName: "", userIds: ["a", "b"], since: 0, video: false, screen: false },
    { kind: "event", id: "event", channelId: "c", title: "", startsAt: NOW + 5 * 60_000, going: 0 },
    { kind: "poll", id: "poll", channelId: "c", channelName: "", messageId: "m", question: "", voters: 0, endsAt: NOW + 10 * 60_000 },
    { kind: "event", id: "later", channelId: "c", title: "", startsAt: NOW + 5 * 3600_000, going: 0 },
  ];
  assert.deepEqual(pickTiles(tiles, new Set(), NOW).map((t) => t.id), ["event", "poll", "voice"]);
  assert.deepEqual(pickTiles(tiles, new Set(["event", "poll"]), NOW).map((t) => t.id), ["voice", "thread"]);
  // An event long over is gone even if nobody hid it.
  assert.deepEqual(pickTiles(tiles, new Set(), NOW + 6 * 3600_000).map((t) => t.id).includes("later"), false);
});

test("a followed thread's parent is found in a loaded channel when its thread was never opened", () => {
  const parent = { id: "t1", channelId: "general", content: "Plans", thread: { replyCount: 2, participantIds: [] } } as unknown as Message;
  const [tile] = collectTiles(sources({ messages: { general: { items: [parent] } }, followed: { t1: true }, threadUnread: { t1: 2 } }), NOW);
  assert.equal(tile?.kind === "thread" && tile.channelId, "general");
});

test("voice room tiles start off in big servers, and a server's own choices win", () => {
  assert.equal(enabledKinds(undefined, 499).has("voice"), true);
  assert.equal(enabledKinds(undefined, 500).has("voice"), false);
  assert.equal(enabledKinds(undefined, 500).has("poll"), true);
  assert.equal(enabledKinds({ voice: true }, 50_000).has("voice"), true);
  assert.deepEqual([...enabledKinds({ poll: false, thread: false }, 10)], ["event", "voice", "shared", "custom"]);
  const tiles: Tile[] = [
    { kind: "voice", id: "voice", channelId: "c", channelName: "", userIds: ["a", "b"], since: 0, video: false, screen: false },
    { kind: "thread", id: "thread", channelId: "c", threadId: "t", title: "", replies: 1, unread: 1, userIds: [] },
  ];
  assert.deepEqual(pickTiles(tiles, new Set(), NOW, enabledKinds(undefined, 2_000)).map((t) => t.id), ["thread"]);
});

test("an app's tile is cut to the template before anyone sees it", () => {
  const fitted = fitCustom({
    kind: "custom",
    id: "c",
    channelId: "general",
    app: "Scorebot",
    title: "A very\nlong title that keeps going well past forty characters",
    status: "67'",
    live: true,
    rows: [1, 2, 3, 4, 5].map((n) => ({ label: `Team ${n}`, value: "123456789" })),
    progress: 7,
    action: "Watch",
  });
  assert.equal(fitted.title.length, 40);
  assert.ok(!fitted.title.includes("\n"));
  assert.equal(fitted.rows.length, 4);
  assert.equal(fitted.rows[0]!.value, "1234567…");
  assert.equal(fitted.progress, 1);
  assert.equal(fitCustom({ ...fitted, progress: Number.NaN }).progress, null);
});
