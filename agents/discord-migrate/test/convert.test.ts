import assert from "node:assert/strict";
import { test } from "node:test";
import { Permission } from "@waifu-devs/fuwa";
import {
  type Mapping,
  chunks,
  dayDivider,
  displayOrder,
  emojiName,
  embedsOf,
  messageText,
  overwritesOf,
  permissionsOf,
  rewrite,
} from "../src/convert.ts";
import type { SourceChannel, SourceMessage } from "../src/model.ts";

const map: Mapping = {
  guildId: "100",
  serverId: "srv",
  roles: { "200": "role-mods" },
  roleNames: { "200": "Mods", "201": "Bots" },
  channels: { "300": "chan-general" },
  channelNames: { "300": "general", "301": "secret" },
  emojis: { "400": { id: "emo-wave", name: "wave" } },
};

test("Discord's permission bits become fuwa's", () => {
  // View Channel, Send Messages, Kick Members.
  const bits = (1n << 10n) | (1n << 11n) | (1n << 1n);
  assert.deepEqual(permissionsOf(bits), [
    Permission.KICK_MEMBERS,
    Permission.VIEW_CHANNELS,
    Permission.SEND_MESSAGES,
    Permission.MANAGE_CARDS,
  ]);
  assert.deepEqual(permissionsOf(1n << 3n), [Permission.ADMINISTRATOR]);
  // Bits fuwa has nothing for are dropped.
  assert.deepEqual(permissionsOf(1n << 8n), []);
});

test("a channel's overwrites keep only what fuwa lets channels change, for roles that came over", () => {
  const overwrites = overwritesOf(
    [
      // @everyone can't see it; Kick Members can't be per channel.
      { id: "100", kind: "role", allow: 1n << 1n, deny: 1n << 10n },
      { id: "200", kind: "role", allow: 1n << 10n, deny: 0n },
      // A role that wasn't copied, and a member.
      { id: "201", kind: "role", allow: 1n << 10n, deny: 0n },
      { id: "999", kind: "member", allow: 1n << 10n, deny: 0n },
    ],
    "100",
    "srv",
    map.roles,
  );
  assert.deepEqual(overwrites, [
    { targetId: "srv", kind: "role", allow: [], deny: [Permission.VIEW_CHANNELS] },
    { targetId: "role-mods", kind: "role", allow: [Permission.VIEW_CHANNELS], deny: [] },
  ]);
});

test("channels come out in Discord's order: loose ones, then each category with its own, voice last", () => {
  const ch = (id: string, kind: SourceChannel["kind"], position: number, parentId?: string): SourceChannel => ({
    id,
    name: id,
    kind,
    parentId,
    position,
    topic: "",
    slowmodeSeconds: 0,
    overwrites: [],
  });
  const order = displayOrder([
    ch("voice-chat", "voice", 0, "cat-b"),
    ch("cat-b", "category", 1),
    ch("rules", "text", 0),
    ch("cat-a", "category", 0),
    ch("lounge", "voice", 0),
    ch("memes", "text", 1, "cat-b"),
    ch("news", "announcement", 0, "cat-a"),
  ]).map((c) => c.id);
  assert.deepEqual(order, ["rules", "lounge", "cat-a", "news", "cat-b", "memes", "voice-chat"]);
});

test("emoji names fit fuwa's rules and don't clash", () => {
  const taken = new Set<string>();
  assert.equal(emojiName("party-parrot", taken), "party_parrot");
  assert.equal(emojiName("party parrot", taken), "party_parrot_2");
  assert.equal(emojiName("x", taken), "emoji_x");
  assert.equal(emojiName("ÿ", taken), "emoji");
  assert.equal(emojiName("a".repeat(40), taken).length, 32);
});

test("messages point at what was copied, and name what wasn't", () => {
  const text = rewrite(
    "hi <@1> and <@!2>, see <#300> and <#301>, ask <@&200> or <@&201> <@&100> <:wave:400> <a:dance:401> </ping:5>",
    { "1": "alice" },
    map,
  );
  assert.equal(
    text,
    "hi @alice and @someone, see <#chan-general> and #secret, ask <@&role-mods> or @Bots @everyone <:wave:emo-wave> :dance: `/ping`",
  );
});

test("a message's text carries its reply and stickers", () => {
  const message: SourceMessage = {
    id: "1",
    authorName: "bob",
    at: 0,
    content: "agreed <@1>",
    system: false,
    pinned: false,
    replyTo: { authorName: "alice", content: "\n> quoted\nsecond line" },
    attachments: [],
    embeds: [],
    stickers: ["Thumbs"],
    mentionedUsers: { "1": "alice" },
  };
  assert.equal(messageText(message, map), "> ↪ **alice**: quoted\nagreed @alice\n*[sticker: Thumbs]*");
});

test("long text is cut at line breaks, each piece short enough", () => {
  const text = `${"a".repeat(30)}\n${"b".repeat(30)}\n${"c".repeat(10)}`;
  assert.deepEqual(chunks(text, 40), ["a".repeat(30), "b".repeat(30), "c".repeat(10)]);
  assert.deepEqual(chunks("x".repeat(90), 40), ["x".repeat(40), "x".repeat(40), "x".repeat(10)]);
  assert.deepEqual(chunks("  "), []);
  assert.deepEqual(chunks("short"), ["short"]);
});

test("embeds keep only web links", () => {
  const [embed] = embedsOf([{ title: "t", url: "javascript:alert(1)", imageUrl: "https://x/y.png", fields: [] }]);
  assert.equal(embed!.url, undefined);
  assert.equal(embed!.imageUrl, "https://x/y.png");
});

test("day dividers are timestamps everyone reads in their own zone", () => {
  assert.equal(dayDivider(1_700_000_000_500), "── <t:1700000000:D> ──");
});
