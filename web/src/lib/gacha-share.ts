import type { GachaBanner, GachaPull, GachaShare } from "@/gen/fuwa/v1/types_pb";
import { OUTCOME_DUPLICATE, OUTCOME_REFUNDED, TIER_COLORS, rarity, stars, tierColor } from "./gacha.ts";

/*
 * A gacha pull to show off (docs/economy.md): the cards a message shares
 * (Message.gacha) and the picture both apps save of them. The picture is
 * laid out here as shapes (an SVG) and words; the desktop's
 * `core/gacha_share.rs` lays it out the same way, tested against the same
 * file (gacha-share.test.svg). Only type imports and pure functions, so
 * node's test runner reads it as is.
 */

/** One card, as it's drawn. */
export type ShareCard = {
  tierName: string;
  prizeName: string;
  rarity: number;
  stars: number;
  /** 0xRRGGBB. */
  color: number;
  outcome: number;
  guaranteed: boolean;
};

export type ShareCards = { bannerName: string; cards: ShareCard[] };

/** What a shared message carries, with each tier's color settled. */
export function cardsOfShare(share: Pick<GachaShare, "bannerName" | "cards">): ShareCards {
  return {
    bannerName: share.bannerName,
    cards: share.cards.map((c) => ({
      tierName: c.tierName,
      prizeName: c.prizeName,
      rarity: c.rarity,
      stars: Math.max(1, c.stars),
      color: c.color ?? TIER_COLORS[Math.min(Math.max(c.tierAt, 0), TIER_COLORS.length - 1)]!,
      outcome: c.outcome,
      guaranteed: c.guaranteed,
    })),
  };
}

/** A pull just made, ranked by its banner as the server ranks a shared one. */
export function cardsOfPull(pull: Pick<GachaPull, "bannerName" | "results">, banner: Pick<GachaBanner, "tiers" | "pityTierId">): ShareCards {
  return {
    bannerName: pull.bannerName,
    cards: pull.results.map((r) => {
      const at = banner.tiers.findIndex((t) => t.id === r.tierId);
      return {
        tierName: r.tierName,
        prizeName: r.prizeName,
        rarity: rarity(banner, r.tierId),
        stars: stars(banner, r.tierId),
        color: at === -1 ? TIER_COLORS[TIER_COLORS.length - 1]! : tierColor(banner.tiers[at]!, at),
        outcome: r.outcome,
        guaranteed: r.guaranteed,
      };
    }),
  };
}

/** The card the most is made of: the rarest, the first of them. */
export function bestCard(cards: readonly ShareCard[]): ShareCard | undefined {
  return cards.reduce<ShareCard | undefined>((best, c) => (!best || c.rarity > best.rarity ? c : best), undefined);
}

/** What the chip under a card says, if anything: how it came out, else that pity made it. */
export function chipKey(card: Pick<ShareCard, "outcome" | "guaranteed">): "refunded" | "duplicate" | "guaranteed" | null {
  if (card.outcome === OUTCOME_REFUNDED) return "refunded";
  if (card.outcome === OUTCOME_DUPLICATE) return "duplicate";
  return card.guaranteed ? "guaranteed" : null;
}

// ── The picture ──────────────────────────────────────────────────────────

/** Words drawn on the picture. */
export type ImageText = {
  x: number;
  y: number;
  size: number;
  weight: 400 | 700 | 800;
  /** #rrggbb. */
  color: string;
  opacity: number;
  anchor: "start" | "middle" | "end";
  text: string;
};

/** The picture: `shapes` is an SVG without words, so a browser can draw it and then write the words in the app's own font. */
export type ShareImage = { width: number; height: number; shapes: string; texts: ImageText[] };

/** Its words, in the reader's language. */
export type ImageLabels = {
  /** Under the banner's name: who pulled, and how many. */
  subtitle: string;
  /** The chip under a card (`chipKey`). */
  chip: (key: "refunded" | "duplicate" | "guaranteed") => string;
};

/** The font the words are in: the apps' own, else any sans-serif one. */
export const IMAGE_FONT = "M PLUS Rounded 1c";
// Its files name it "Rounded Mplus 1c", as the desktop finds it.
const FONT_STACK = `'${IMAGE_FONT}', 'Rounded Mplus 1c', sans-serif`;

const PAD = 40;
const HEAD = 120;
const CARD_W = 160;
const CARD_H = 216;
const GAP = 16;
const FOOT = 40;
const COLS = 5;
const NIGHT = "#0b0a14";
const FACE = "#15131f";
const WHITE = "#ffffff";

/** A number as the SVG writes it: to two places, as short as it goes. */
export function num(v: number): string {
  const r = Math.round(v * 100) / 100;
  return String(r === 0 ? 0 : r);
}

export const hex = (color: number): string => `#${(color & 0xffffff).toString(16).padStart(6, "0")}`;

export function escapeXml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&apos;");
}

/** At most `most` characters, the last an ellipsis when it's cut. */
export function clip(text: string, most: number): string {
  const chars = Array.from(text);
  return chars.length <= most ? text : `${chars.slice(0, most - 1).join("")}…`;
}

/** A five-pointed star around (cx, cy). */
function star(cx: number, cy: number, outer: number, inner: number): string {
  const points: string[] = [];
  for (let i = 0; i < 10; i++) {
    const r = i % 2 === 0 ? outer : inner;
    const a = (Math.PI / 5) * i - Math.PI / 2;
    points.push(`${num(cx + r * Math.cos(a))},${num(cy + r * Math.sin(a))}`);
  }
  return points.join(" ");
}

/** The four-pointed sparkle in the middle of a card. */
function sparkle(cx: number, cy: number, r: number): string {
  const k = r * 0.28;
  return `M${num(cx)} ${num(cy - r)}Q${num(cx + k)} ${num(cy - k)} ${num(cx + r)} ${num(cy)}Q${num(cx + k)} ${num(cy + k)} ${num(cx)} ${num(cy + r)}Q${num(cx - k)} ${num(cy + k)} ${num(cx - r)} ${num(cy)}Q${num(cx - k)} ${num(cy - k)} ${num(cx)} ${num(cy - r)}Z`;
}

/** Lays out the picture of a pull's cards. */
export function shareImage(pulled: ShareCards, labels: ImageLabels): ShareImage {
  const n = pulled.cards.length;
  const cols = Math.max(1, Math.min(COLS, n));
  const rows = Math.max(1, Math.ceil(n / cols));
  const width = Math.max(480, PAD * 2 + cols * CARD_W + (cols - 1) * GAP);
  const height = HEAD + rows * CARD_H + (rows - 1) * GAP + FOOT + PAD / 2;
  const glow = hex(bestCard(pulled.cards)?.color ?? TIER_COLORS[0]!);
  const defs: string[] = [
    `<radialGradient id="glow" cx="50%" cy="0%" r="75%"><stop offset="0" stop-color="${glow}" stop-opacity="0.45"/><stop offset="1" stop-color="${glow}" stop-opacity="0"/></radialGradient>`,
  ];
  const body: string[] = [`<rect width="${width}" height="${height}" rx="32" fill="${NIGHT}"/>`, `<rect width="${width}" height="${height}" rx="32" fill="url(#glow)"/>`];
  const texts: ImageText[] = [
    { x: PAD, y: 66, size: 32, weight: 800, color: WHITE, opacity: 1, anchor: "start", text: clip(pulled.bannerName, 40) },
    { x: PAD, y: 98, size: 17, weight: 400, color: WHITE, opacity: 0.65, anchor: "start", text: labels.subtitle },
  ];
  pulled.cards.forEach((card, i) => {
    const row = Math.floor(i / cols);
    const inRow = Math.min(cols, n - row * cols);
    const rowW = inRow * CARD_W + (inRow - 1) * GAP;
    const x = (width - rowW) / 2 + (i % cols) * (CARD_W + GAP);
    const y = HEAD + row * (CARD_H + GAP);
    const cx = x + CARD_W / 2;
    const color = hex(card.color);
    defs.push(
      `<linearGradient id="c${i}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${color}" stop-opacity="0.45"/><stop offset="0.6" stop-color="${color}" stop-opacity="0"/></linearGradient>`,
    );
    if (card.rarity >= 3) body.push(`<rect x="${num(x - 6)}" y="${num(y - 6)}" width="${CARD_W + 12}" height="${CARD_H + 12}" rx="24" fill="${color}" fill-opacity="0.28"/>`);
    body.push(`<rect x="${num(x)}" y="${num(y)}" width="${CARD_W}" height="${CARD_H}" rx="18" fill="${FACE}"/>`);
    body.push(`<rect x="${num(x)}" y="${num(y)}" width="${CARD_W}" height="${CARD_H}" rx="18" fill="url(#c${i})" stroke="${color}" stroke-width="3"/>`);
    const count = Math.max(1, Math.min(5, card.stars));
    for (let s = 0; s < count; s++) body.push(`<polygon points="${star(cx + (s - (count - 1) / 2) * 18, y + 26, 8, 3.4)}" fill="${color}"/>`);
    body.push(`<circle cx="${num(cx)}" cy="${num(y + 88)}" r="34" fill="${color}" fill-opacity="0.22"/>`);
    body.push(`<path d="${sparkle(cx, y + 88, card.rarity >= 2 ? 22 : 16)}" fill="${color}"/>`);
    texts.push({ x: cx, y: y + 156, size: 15, weight: 700, color: WHITE, opacity: 1, anchor: "middle", text: clip(card.prizeName, 17) });
    texts.push({ x: cx, y: y + 178, size: 12, weight: 800, color, opacity: 1, anchor: "middle", text: clip(card.tierName.toUpperCase(), 18) });
    const chip = chipKey(card);
    if (chip) texts.push({ x: cx, y: y + 200, size: 11, weight: 700, color: WHITE, opacity: 0.7, anchor: "middle", text: clip(labels.chip(chip), 20) });
  });
  texts.push({ x: width - PAD, y: height - 24, size: 14, weight: 800, color: WHITE, opacity: 0.4, anchor: "end", text: "fuwa" });
  const shapes = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}"><defs>${defs.join("")}</defs>${body.join("")}</svg>`;
  return { width, height, shapes, texts };
}

/** The whole picture as one SVG, words and all: what the desktop draws. */
export function imageSvg(image: ShareImage): string {
  const words = image.texts
    .map(
      (t) =>
        `<text x="${num(t.x)}" y="${num(t.y)}" font-family="${FONT_STACK}" font-size="${t.size}" font-weight="${t.weight}" fill="${t.color}"${t.opacity < 1 ? ` fill-opacity="${num(t.opacity)}"` : ""} text-anchor="${t.anchor}">${escapeXml(t.text)}</text>`,
    )
    .join("");
  return image.shapes.replace(/<\/svg>$/, `${words}</svg>`);
}
