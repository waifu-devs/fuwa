import assert from "node:assert/strict";
import { test } from "node:test";
import type { Emoji, Server } from "@/gen/fuwa/v1/types_pb";
import { catalogOf, choiceName, encodeEmoji, outsideEmojis, searchCatalog, toned, type StandardGroup } from "./emoji-search.ts";

const server = (id: string, name: string) => ({ id, name, iconUrl: "" }) as Server;
const emoji = (id: string, serverId: string, name: string, animated = false) =>
  ({ id, serverId, name, url: `https://chat.example/media/${id}`, animated }) as Emoji;

const HERE = server("S1", "Here");
const OTHER = server("S2", "Other");
const THIRD = server("S3", "Third");
const EMOJIS: Record<string, Emoji[]> = {
  S1: [emoji("E1aaaaaaaaaa", "S1", "wave")],
  S2: [emoji("E2aaaaaaaaaa", "S2", "wave"), emoji("E3aaaaaaaaaa", "S2", "blobcat", true)],
  S3: [emoji("E4aaaaaaaaaa", "S3", "Wave")],
};
const STANDARD: StandardGroup[] = [
  {
    id: "people",
    name: "Smileys & people",
    emojis: [
      { char: "👋", names: ["wave"], words: "hand hello", skins: ["👋🏻", "👋🏼", "👋🏽", "👋🏾", "👋🏿"] },
      { char: "😂", names: ["joy", "tears_of_joy"], words: "laugh lol" },
    ],
  },
];

test("this server's emoji come first and keep their names; others with the same name get ~2, ~3", () => {
  const catalog = catalogOf([OTHER, HERE, THIRD], EMOJIS, "S1");
  assert.deepEqual(
    catalog.sections.map((s) => [s.server.id, s.here, s.emojis.map((e) => e.alias)]),
    [
      ["S1", true, ["wave"]],
      ["S2", false, ["blobcat", "wave~2"]],
      ["S3", false, ["Wave~3"]],
    ],
  );
  assert.equal(catalog.byAlias.get("wave~2")?.emoji.id, "E2aaaaaaaaaa");
});

test(":name: becomes the token of the emoji that name finds, case aside, and nothing else changes", () => {
  const catalog = catalogOf([HERE, OTHER, THIRD], EMOJIS, "S1");
  assert.equal(
    encodeEmoji("hi :wave: :WAVE~2: :blobcat: :wave~3: :nope: <:wave:E1aaaaaaaaaa>", catalog),
    "hi <:wave:E1aaaaaaaaaa> <:wave:E2aaaaaaaaaa> <a:blobcat:E3aaaaaaaaaa> <:Wave:E4aaaaaaaaaa> :nope: <:wave:E1aaaaaaaaaa>",
  );
});

test("only other servers' emoji go along with a message, once each", () => {
  const sent = outsideEmojis(EMOJIS, "S1", "<:wave:E1aaaaaaaaaa> <:wave:E2aaaaaaaaaa> <a:blobcat:E3aaaaaaaaaa> <a:blobcat:E3aaaaaaaaaa> <:gone:E9aaaaaaaaaa>");
  assert.deepEqual(
    sent.map((e) => e.id),
    ["E2aaaaaaaaaa", "E3aaaaaaaaaa"],
  );
  assert.deepEqual(outsideEmojis(EMOJIS, "S1", "no emoji"), []);
});

test("search ranks whole names, then starts, then the rest; this server, other servers, then standard", () => {
  const catalog = catalogOf([HERE, OTHER], EMOJIS, "S1");
  assert.deepEqual(searchCatalog(":wave", catalog, STANDARD).map(choiceName), ["wave", "wave~2", "wave"]);
  assert.deepEqual(searchCatalog("bl", catalog, STANDARD).map(choiceName), ["blobcat"]);
  // Words find standard ones last; a name inside another is still found.
  assert.deepEqual(searchCatalog("lau", catalog, STANDARD).map(choiceName), ["joy"]);
  assert.deepEqual(searchCatalog("of_joy", catalog, STANDARD).map(choiceName), ["joy"]);
  assert.deepEqual(searchCatalog("  ", catalog, STANDARD), []);
  assert.equal(searchCatalog("a", catalog, STANDARD, 1).length, 1);
});

test("skin tones apply only to emoji that have them", () => {
  const [wave, joy] = STANDARD[0]!.emojis;
  assert.equal(toned(wave!, 0), "👋");
  assert.equal(toned(wave!, 3), "👋🏽");
  assert.equal(toned(joy!, 3), "😂");
});
