import type { GachaBanner, GachaProgress, GachaPrize, GachaTier, ShopListing } from "@/gen/fuwa/v1/types_pb";
import type { Key } from "../i18n/i18n.ts";

/*
 * Gacha banners (docs/economy.md): the small decisions the shop and the
 * banner editor share, from showing a rate as a percentage to what a
 * banner draft sends. Only type imports, so node's test runner reads this
 * file as is; the enums it needs are written out below, as the protocol
 * numbers them.
 */

/** fuwa.v1.GachaDuplicates. */
export const DUPLICATES_KEEP = 1;
export const DUPLICATES_REFUND = 2;
export const DUPLICATES_SKIP_OWNED = 3;

/** fuwa.v1.GachaOutcome. */
export const OUTCOME_WON = 1;
export const OUTCOME_DUPLICATE = 2;
export const OUTCOME_REFUNDED = 3;

/** A rate of 100%, in millionths, as the server counts them. */
export const WHOLE = 1_000_000;

/** As the server takes them. */
export const BANNER_NAME_MAX = 60;
export const BANNER_DESCRIPTION_MAX = 500;
export const TIERS_MAX = 10;
export const TIER_NAME_MAX = 32;
export const PRIZES_MAX = 100;
export const MULTI_MAX = 100;
export const WEIGHT_MAX = 1_000_000;
export const PITY_MAX = 10_000;
export const MAX_PULLS = 1_000_000;
export const PRIZE_MAX = 1_000_000_000;
const PRICE_MAX = 1_000_000_000_000;

const num = (v: bigint | number | undefined): number => Number(v ?? 0);
const ms = (at: { seconds: bigint; nanos: number } | undefined): number => (at ? Number(at.seconds) * 1000 + Math.floor(at.nanos / 1e6) : 0);

function whole(text: string, min: number, max: number): number | null {
  const plain = text.replace(/[\s,_']/g, "");
  if (!/^\d+$/.test(plain)) return null;
  const n = Number(plain);
  return Number.isSafeInteger(n) && n >= min && n <= max ? n : null;
}

// ── Rates ────────────────────────────────────────────────────────────────

/** A rate in millionths as a percentage, with as many decimals as it needs (up to four): "0.6%", "12.5%", "100%". */
export function percent(rate: number): string {
  const value = (rate / WHOLE) * 100;
  return `${Number(value.toFixed(4))}%`;
}

/** A percentage someone typed ("0.6", "12,5 %") in millionths, or null when it isn't 0 to 100 with at most four decimals. */
export function parsePercent(text: string): number | null {
  const plain = text.replace(/[\s%]/g, "").replace(",", ".");
  if (!/^\d+(\.\d{1,4})?$/.test(plain) && !/^\.\d{1,4}$/.test(plain)) return null;
  const rate = Math.round(Number(plain) * 10_000);
  return rate >= 0 && rate <= WHOLE ? rate : null;
}

/** A banner's tiers, rarest first, each with the prizes that can drop in it. */
export function ratesTable<T extends Pick<GachaTier, "id">, P extends Pick<GachaPrize, "tierId">>(banner: { tiers: readonly T[]; prizes: readonly P[] }): { tier: T; prizes: P[] }[] {
  return banner.tiers.map((tier) => ({ tier, prizes: banner.prizes.filter((p) => p.tierId === tier.id) }));
}

/** What a tier with no color of its own shows, by how rare it is (0 rarest), as both apps pick them. */
export const TIER_COLORS = [0xf59e0b, 0xa855f7, 0x3b82f6, 0x10b981, 0x64748b];
export const tierColor = (tier: Pick<GachaTier, "color">, index: number): number => tier.color ?? TIER_COLORS[Math.min(index, TIER_COLORS.length - 1)]!;

// ── A banner for members ─────────────────────────────────────────────────

export type Openness = { state: "open" | "soon" | "closed"; at: number };

/** Whether a banner can be pulled on now; when it opens or closes (0 for never). */
export function openness(banner: Pick<GachaBanner, "startsAt" | "endsAt">, now: number): Openness {
  const starts = ms(banner.startsAt);
  const ends = ms(banner.endsAt);
  if (starts && starts > now) return { state: "soon", at: starts };
  if (ends && ends <= now) return { state: "closed", at: ends };
  return { state: "open", at: ends };
}

/** The banners members see: available ones in currencies still there, not closed. */
export const shownBanners = <T extends Pick<GachaBanner, "available" | "currencyId" | "startsAt" | "endsAt">>(banners: readonly T[], currencyIds: readonly string[], now: number): T[] =>
  banners.filter((b) => b.available && currencyIds.includes(b.currencyId) && openness(b, now).state !== "closed");

/** What `count` pulls cost (1, or the multi-pull), or null for a count the banner doesn't offer. */
export function pullCost(banner: Pick<GachaBanner, "pullPrice" | "multiPull" | "multiPullPrice">, count: number): number | null {
  if (count === 1) return num(banner.pullPrice);
  if (banner.multiPull >= 2 && count === banner.multiPull) return num(banner.multiPullPrice) || num(banner.pullPrice) * count;
  return null;
}

/** How many pulls until pity's guarantee (1 means the next one), or null without pity. */
export function pityIn(banner: Pick<GachaBanner, "pityPulls">, progress: Pick<GachaProgress, "sincePity"> | undefined): number | null {
  if (!banner.pityPulls) return null;
  return Math.max(1, banner.pityPulls - (progress?.sincePity ?? 0));
}

/** How many more pulls someone may make, or null for no limit. */
export const pullsLeft = (banner: Pick<GachaBanner, "maxPulls">, progress: Pick<GachaProgress, "pulls"> | undefined): number | null =>
  banner.maxPulls ? Math.max(0, banner.maxPulls - (progress?.pulls ?? 0)) : null;

/** The banners a listing drops from, so a gacha-only listing can say where to get it. */
export const bannersWith = <T extends Pick<GachaBanner, "prizes">>(banners: readonly T[], listingId: string): T[] => banners.filter((b) => b.prizes.some((p) => p.listingId === listingId));

/** A banner list with one banner's progress as it is now. */
export function withProgress(progress: Record<string, GachaProgress>, next: GachaProgress | undefined): Record<string, GachaProgress> {
  if (!next) return progress;
  const had = progress[next.bannerId];
  if (had && had.pulls > next.pulls) return progress;
  return { ...progress, [next.bannerId]: next };
}

/** How a draw's outcome reads in the reveal. */
export function outcomeLabel(outcome: number): Key {
  switch (outcome) {
    case OUTCOME_DUPLICATE:
      return "chattools.economy.gacha.duplicate";
    case OUTCOME_REFUNDED:
      return "chattools.economy.gacha.refunded";
    default:
      return "chattools.economy.gacha.new";
  }
}

// ── The reveal ───────────────────────────────────────────────────────────

/** How grand a tier's moment is, 0 to 3: the rarest tier is 3, the pity tier and any rarer 2, the commonest 0, the rest 1. */
export function rarity(banner: Pick<GachaBanner, "tiers" | "pityTierId">, tierId: string): number {
  const n = banner.tiers.length;
  const at = banner.tiers.findIndex((t) => t.id === tierId);
  if (at === -1) return 0;
  if (n === 1) return 1;
  if (at === 0) return 3;
  const pity = banner.pityTierId ? banner.tiers.findIndex((t) => t.id === banner.pityTierId) : -1;
  if (pity !== -1 && at <= pity) return 2;
  return at === n - 1 ? 0 : 1;
}

/** The stars a tier's cards wear: the commonest one, one more for each tier rarer, at most five. */
export function stars(banner: Pick<GachaBanner, "tiers">, tierId: string): number {
  const at = banner.tiers.findIndex((t) => t.id === tierId);
  return at === -1 ? 1 : Math.min(5, banner.tiers.length - at);
}

/** The rarest tier among what dropped (its index, rarest 0), so the pack can glow its color before it opens. */
export function bestTier(banner: Pick<GachaBanner, "tiers">, results: readonly { tierId: string }[]): number {
  let best = banner.tiers.length;
  for (const r of results) {
    const at = banner.tiers.findIndex((t) => t.id === r.tierId);
    if (at !== -1 && at < best) best = at;
  }
  return best === banner.tiers.length ? Math.max(0, best - 1) : best;
}

/** When each part of a pull's reveal happens, in milliseconds from the pull: both apps play it to this clock. */
export type RevealPlan = { burst: number; deal: number; flips: number[]; done: number };

/** The pack charges, bursts, the cards are dealt face down, then turn one by one, waiting a beat before a rare one and after the rarest. Reduced motion shows them all at once. */
export const REVEAL = { charge: 1300, burst: 350, deal: 60, settle: 320, flip: 220, suspense: 380, linger: 320 } as const;

export function revealPlan(rarities: readonly number[], reduced: boolean): RevealPlan {
  if (reduced) return { burst: 0, deal: 0, flips: rarities.map(() => 0), done: 0 };
  const burst = REVEAL.charge;
  const deal = burst + REVEAL.burst;
  let at = deal + rarities.length * REVEAL.deal + REVEAL.settle;
  const flips = rarities.map((r, n) => {
    if (n > 0) at += REVEAL.flip;
    if (r >= 2) at += REVEAL.suspense;
    const flip = at;
    if (r >= 3) at += REVEAL.linger;
    return flip;
  });
  return { burst, deal, flips, done: at + REVEAL.flip };
}

// ── The banner editor ────────────────────────────────────────────────────

export type TierDraft = { id: string; name: string; color: number | null; rate: string };
export type PrizeDraft = { id: string; tierId: string; listingId: string; currencyId: string; amount: string; weight: string };
export type BannerDraft = {
  name: string;
  description: string;
  /** The box or card pack's picture, uploaded for the server; "" for none. */
  artUrl: string;
  currencyId: string;
  price: string;
  /** "" for no multi-pull. */
  multi: string;
  /** "" for the single price times the count. */
  multiPrice: string;
  tiers: TierDraft[];
  prizes: PrizeDraft[];
  /** "" for no pity. */
  pityPulls: string;
  pityTierId: string;
  duplicates: number;
  refundCurrencyId: string;
  refundAmount: string;
  /** "" for no limit. */
  maxPulls: string;
  /** Local date and time as a datetime-local input has it; "" for none. */
  startsAt: string;
  endsAt: string;
  available: boolean;
  position: string;
};

/** A new tier's id: short, and not one the banner has. */
export function newTierId(tiers: readonly Pick<TierDraft, "id">[]): string {
  for (let n = tiers.length + 1; ; n++) {
    const id = `t${n}`;
    if (!tiers.some((t) => t.id === id)) return id;
  }
}

/** A moment as a datetime-local input shows it, in the viewer's time zone. */
export function localInput(at: number): string {
  if (!at) return "";
  const d = new Date(at);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}T${two(d.getHours())}:${two(d.getMinutes())}`;
}

/** What a datetime-local input holds, as a moment, or null when it's empty or not a date. */
export function fromLocalInput(text: string): number | null {
  if (!text.trim()) return null;
  const at = new Date(text).getTime();
  return Number.isFinite(at) ? at : null;
}

/** The tiers a new banner starts with. */
export const STARTER_TIERS: TierDraft[] = [
  { id: "t1", name: "Legendary", color: TIER_COLORS[0]!, rate: "3" },
  { id: "t2", name: "Rare", color: TIER_COLORS[1]!, rate: "17" },
  { id: "t3", name: "Common", color: TIER_COLORS[2]!, rate: "80" },
];

const text = (n: number | bigint) => (num(n) ? String(num(n)) : "");

/** A banner as the editor holds it; a new one starts with three tiers, a 10-pull and no prizes. */
export function bannerDraft(b: GachaBanner | undefined, currencyId = "", position = 0, starterNames?: [string, string, string]): BannerDraft {
  if (!b) {
    const tiers = STARTER_TIERS.map((t, i) => ({ ...t, name: starterNames?.[i] ?? t.name }));
    return {
      name: "",
      description: "",
      artUrl: "",
      currencyId,
      price: "100",
      multi: "10",
      multiPrice: "",
      tiers,
      prizes: [],
      pityPulls: "",
      pityTierId: tiers[0]!.id,
      duplicates: DUPLICATES_KEEP,
      refundCurrencyId: currencyId,
      refundAmount: "",
      maxPulls: "",
      startsAt: "",
      endsAt: "",
      available: false,
      position: String(position),
    };
  }
  return {
    name: b.name,
    description: b.description,
    artUrl: b.artUrl,
    currencyId: b.currencyId,
    price: String(num(b.pullPrice)),
    multi: text(b.multiPull),
    multiPrice: text(b.multiPullPrice),
    tiers: b.tiers.map((t) => ({ id: t.id, name: t.name, color: t.color ?? null, rate: String(Number(((t.rate / WHOLE) * 100).toFixed(4))) })),
    prizes: b.prizes.map((p) => ({ id: p.id, tierId: p.tierId, listingId: p.listingId, currencyId: p.currencyId, amount: text(p.amount), weight: String(p.weight) })),
    pityPulls: text(b.pityPulls),
    pityTierId: b.pityTierId || (b.tiers[0]?.id ?? ""),
    duplicates: b.duplicates || DUPLICATES_KEEP,
    refundCurrencyId: b.refundCurrencyId || b.currencyId,
    refundAmount: text(b.refundAmount),
    maxPulls: text(b.maxPulls),
    startsAt: localInput(ms(b.startsAt)),
    endsAt: localInput(ms(b.endsAt)),
    available: b.available,
    position: String(b.position),
  };
}

/** The sum of the tiers' rates as typed, in millionths; null when one isn't a percentage. */
export function ratesTotal(tiers: readonly Pick<TierDraft, "rate">[]): number | null {
  let total = 0;
  for (const t of tiers) {
    const rate = parsePercent(t.rate);
    if (rate === null) return null;
    total += rate;
  }
  return total;
}

/** Each prize's chance as the draft stands, in millionths (0 where a number isn't one yet). */
export function draftChances(draft: Pick<BannerDraft, "tiers" | "prizes">): Record<string, number> {
  const out: Record<string, number> = {};
  for (const tier of draft.tiers) {
    const rate = parsePercent(tier.rate) ?? 0;
    const inTier = draft.prizes.filter((p) => p.tierId === tier.id);
    const weights = inTier.map((p) => whole(p.weight, 1, WEIGHT_MAX) ?? 0);
    const total = weights.reduce((a, b) => a + b, 0);
    inTier.forEach((p, i) => {
      out[p.id] = total ? Math.floor((rate * weights[i]!) / total) : 0;
    });
  }
  return out;
}

export type BannerProblem =
  | "name"
  | "currency"
  | "price"
  | "multi"
  | "multiPrice"
  | "tiers"
  | "tierName"
  | "tierRate"
  | "ratesTotal"
  | "prizes"
  | "prizeWhat"
  | "prizeAmount"
  | "prizeWeight"
  | "emptyTier"
  | "pity"
  | "refund"
  | "maxPulls"
  | "dates"
  | "position";

/** The plain shape of a GachaBanner to send, with its times as moments. */
export type BannerValue = {
  id: string;
  name: string;
  description: string;
  artUrl: string;
  currencyId: string;
  pullPrice: bigint;
  multiPull: number;
  multiPullPrice: bigint;
  tiers: { id: string; name: string; color?: number; rate: number }[];
  prizes: { id: string; tierId: string; listingId: string; currencyId: string; amount: bigint; weight: number }[];
  pityPulls: number;
  pityTierId: string;
  duplicates: number;
  refundCurrencyId: string;
  refundAmount: bigint;
  maxPulls: number;
  startsAtMs: number | null;
  endsAtMs: number | null;
  available: boolean;
  position: number;
};

/** What to send for a draft, or the first thing that's wrong with it (and which tier or prize, by index). */
export function bannerValue(draft: BannerDraft, id = ""): { banner: BannerValue } | { problem: BannerProblem; index?: number } {
  const name = draft.name.trim();
  if (!name || [...name].length > BANNER_NAME_MAX) return { problem: "name" };
  if (!draft.currencyId) return { problem: "currency" };
  const price = whole(draft.price, 1, PRICE_MAX);
  if (price === null) return { problem: "price" };
  const multi = draft.multi.trim() === "" ? 0 : whole(draft.multi, 2, MULTI_MAX);
  if (multi === null) return { problem: "multi" };
  const multiPrice = !multi || draft.multiPrice.trim() === "" ? 0 : whole(draft.multiPrice, 1, PRICE_MAX);
  if (multiPrice === null) return { problem: "multiPrice" };
  if (!draft.tiers.length || draft.tiers.length > TIERS_MAX) return { problem: "tiers" };
  const tiers: BannerValue["tiers"] = [];
  for (const [index, t] of draft.tiers.entries()) {
    const tierName = t.name.trim();
    if (!tierName || [...tierName].length > TIER_NAME_MAX) return { problem: "tierName", index };
    const rate = parsePercent(t.rate);
    if (rate === null) return { problem: "tierRate", index };
    tiers.push({ id: t.id, name: tierName, ...(t.color !== null && { color: t.color }), rate });
  }
  if (tiers.reduce((a, t) => a + t.rate, 0) !== WHOLE) return { problem: "ratesTotal" };
  if (!draft.prizes.length || draft.prizes.length > PRIZES_MAX) return { problem: "prizes" };
  const prizes: BannerValue["prizes"] = [];
  for (const [index, p] of draft.prizes.entries()) {
    if (!p.listingId && !p.currencyId) return { problem: "prizeWhat", index };
    const amount = p.listingId ? 0 : whole(p.amount, 1, PRIZE_MAX);
    if (amount === null) return { problem: "prizeAmount", index };
    const weight = whole(p.weight, 1, WEIGHT_MAX);
    if (weight === null) return { problem: "prizeWeight", index };
    prizes.push({ id: p.id.startsWith("new:") ? "" : p.id, tierId: p.tierId, listingId: p.listingId, currencyId: p.listingId ? "" : p.currencyId, amount: BigInt(amount), weight });
  }
  const empty = tiers.findIndex((t) => t.rate > 0 && !prizes.some((p) => p.tierId === t.id));
  if (empty !== -1) return { problem: "emptyTier", index: empty };
  const pity = draft.pityPulls.trim() === "" ? 0 : whole(draft.pityPulls, 1, PITY_MAX);
  if (pity === null || (pity && !tiers.some((t) => t.id === draft.pityTierId))) return { problem: "pity" };
  const refund = draft.duplicates === DUPLICATES_REFUND ? whole(draft.refundAmount, 1, PRIZE_MAX) : 0;
  if (refund === null || (draft.duplicates === DUPLICATES_REFUND && !draft.refundCurrencyId)) return { problem: "refund" };
  const maxPulls = draft.maxPulls.trim() === "" ? 0 : whole(draft.maxPulls, 1, MAX_PULLS);
  if (maxPulls === null) return { problem: "maxPulls" };
  const startsAt = fromLocalInput(draft.startsAt);
  const endsAt = fromLocalInput(draft.endsAt);
  if ((draft.startsAt.trim() && startsAt === null) || (draft.endsAt.trim() && endsAt === null) || (startsAt && endsAt && endsAt <= startsAt)) return { problem: "dates" };
  const position = /^-?\d+$/.test(draft.position.trim() || "0") ? Number(draft.position.trim() || "0") : null;
  if (position === null || Math.abs(position) > 1_000_000) return { problem: "position" };
  return {
    banner: {
      id,
      name,
      description: draft.description.trim(),
      artUrl: draft.artUrl,
      currencyId: draft.currencyId,
      pullPrice: BigInt(price),
      multiPull: multi,
      multiPullPrice: BigInt(multiPrice),
      tiers,
      prizes,
      pityPulls: pity,
      pityTierId: pity ? draft.pityTierId : "",
      duplicates: draft.duplicates,
      refundCurrencyId: draft.duplicates === DUPLICATES_REFUND ? draft.refundCurrencyId : "",
      refundAmount: BigInt(refund),
      maxPulls,
      startsAtMs: startsAt,
      endsAtMs: endsAt,
      available: draft.available,
      position,
    },
  };
}

/** Whether a draft says something other than the banner saved (or there's none saved yet). */
export function draftChanged(saved: GachaBanner | undefined, draft: BannerDraft): boolean {
  if (!saved) return true;
  return JSON.stringify(bannerDraft(saved)) !== JSON.stringify(draft);
}

/** Listings that can be prizes, by kind as the shop lists them: every one, gacha-only first. */
export const prizeListings = <T extends Pick<ShopListing, "gachaOnly" | "position" | "id">>(shop: readonly T[]): T[] =>
  [...shop].sort((a, b) => Number(b.gachaOnly) - Number(a.gachaOnly) || a.position - b.position || (a.id < b.id ? -1 : 1));
