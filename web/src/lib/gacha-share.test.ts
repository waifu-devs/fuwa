import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { GachaBanner, GachaPull, GachaShare } from "@/gen/fuwa/v1/types_pb";
import { OUTCOME_REFUNDED, OUTCOME_WON } from "./gacha.ts";
import { bestCard, cardsOfPull, cardsOfShare, chipKey, clip, escapeXml, imageSvg, num, shareImage, type ShareCards } from "./gacha-share.ts";

const banner = {
  tiers: [
    { id: "ssr", name: "Legendary", color: undefined, rate: 20_000 },
    { id: "sr", name: "Epic", color: 0x123456, rate: 80_000 },
    { id: "r", name: "Common", color: undefined, rate: 900_000 },
  ],
  pityTierId: "sr",
} as unknown as GachaBanner;

test("a pull and a shared one come out as the same cards", () => {
  const pull = {
    bannerName: "Sakura <Festival>",
    results: [
      { tierId: "r", tierName: "Common", prizeName: "50 Petals", outcome: OUTCOME_WON, guaranteed: false },
      { tierId: "ssr", tierName: "Legendary", prizeName: "Radiant", outcome: OUTCOME_WON, guaranteed: true },
      { tierId: "sr", tierName: "Epic", prizeName: "Holo", outcome: OUTCOME_REFUNDED, guaranteed: false },
    ],
  } as unknown as GachaPull;
  const fromPull = cardsOfPull(pull, banner);
  assert.deepEqual(
    fromPull.cards.map((c) => [c.rarity, c.stars, c.color]),
    [
      [0, 1, 0x3b82f6],
      [3, 3, 0xf59e0b],
      [2, 2, 0x123456],
    ],
  );
  // What the server copies into the message (tier_at for a tier without its own color).
  const share = {
    bannerName: pull.bannerName,
    cards: [
      { tierName: "Common", prizeName: "50 Petals", rarity: 0, stars: 1, tierAt: 2, color: undefined, outcome: OUTCOME_WON, guaranteed: false },
      { tierName: "Legendary", prizeName: "Radiant", rarity: 3, stars: 3, tierAt: 0, color: undefined, outcome: OUTCOME_WON, guaranteed: true },
      { tierName: "Epic", prizeName: "Holo", rarity: 2, stars: 2, tierAt: 1, color: 0x123456, outcome: OUTCOME_REFUNDED, guaranteed: false },
    ],
  } as unknown as GachaShare;
  assert.deepEqual(cardsOfShare(share), fromPull);
  assert.equal(bestCard(fromPull.cards)?.prizeName, "Radiant");
  assert.deepEqual(fromPull.cards.map(chipKey), [null, "guaranteed", "refunded"]);
});

test("the picture's little rules", () => {
  assert.deepEqual([num(1.005), num(12), num(-0.001), num(3.456)], ["1", "12", "0", "3.46"]);
  assert.equal(escapeXml(`<a & 'b'>"`), "&lt;a &amp; &apos;b&apos;&gt;&quot;");
  assert.equal(clip("Custom Emoji Slot Deluxe", 16), "Custom Emoji Sl…");
  assert.equal(clip("Holo", 16), "Holo");
});

/** The pull both apps' tests draw: the desktop's `core/gacha_share.rs` writes the same file. */
const FIXTURE: ShareCards = {
  bannerName: "Sakura <Festival> & Friends",
  cards: [
    { tierName: "Common", prizeName: "50 Sakura Petals", rarity: 0, stars: 1, color: 0x10b981, outcome: OUTCOME_WON, guaranteed: false },
    { tierName: "Rare", prizeName: "Custom Emoji Slot Deluxe", rarity: 1, stars: 2, color: 0x3b82f6, outcome: OUTCOME_WON, guaranteed: false },
    { tierName: "Epic", prizeName: "Night Owl", rarity: 2, stars: 3, color: 0xa855f7, outcome: OUTCOME_REFUNDED, guaranteed: true },
    { tierName: "Legendary", prizeName: "Radiant", rarity: 3, stars: 4, color: 0xf59e0b, outcome: OUTCOME_WON, guaranteed: false },
    { tierName: "Common", prizeName: "Pin Request", rarity: 0, stars: 1, color: 0x10b981, outcome: OUTCOME_WON, guaranteed: false },
    { tierName: "Rare", prizeName: "200 Tokens", rarity: 1, stars: 2, color: 0x3b82f6, outcome: OUTCOME_WON, guaranteed: false },
  ],
};

test("the picture of a pull is laid out as the desktop lays it out", () => {
  const labels = { subtitle: "alice · 6 pulls", chip: (k: string) => ({ refunded: "Refunded", duplicate: "Already had it", guaranteed: "Guaranteed" })[k]! };
  const image = shareImage(FIXTURE, labels);
  assert.equal(image.width, 40 * 2 + 5 * 160 + 4 * 16);
  assert.equal(image.height, 120 + 2 * 216 + 16 + 40 + 20);
  const expected = readFileSync(new URL("./gacha-share.test.svg", import.meta.url), "utf8").trim();
  assert.equal(imageSvg(image), expected);
  // One card alone keeps the picture wide enough for its words.
  assert.equal(shareImage({ bannerName: "x", cards: FIXTURE.cards.slice(0, 1) }, labels).width, 480);
});
