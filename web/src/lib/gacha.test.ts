import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { GachaBanner, GachaProgress, ShopListing } from "@/gen/fuwa/v1/types_pb";
import { economyProblem } from "./economy.ts";
import {
  bannerDraft,
  bannersWith,
  bestTier,
  bannerValue,
  draftChanged,
  draftChances,
  DUPLICATES_KEEP,
  DUPLICATES_REFUND,
  fromLocalInput,
  localInput,
  newTierId,
  openness,
  OUTCOME_DUPLICATE,
  OUTCOME_REFUNDED,
  OUTCOME_WON,
  outcomeLabel,
  parsePercent,
  percent,
  pityIn,
  prizeListings,
  pullCost,
  pullsLeft,
  ratesTable,
  ratesTotal,
  rarity,
  revealPlan,
  shownBanners,
  stars,
  tierColor,
  TIER_COLORS,
  WHOLE,
  withProgress,
  type BannerDraft,
} from "./gacha.ts";

const catalog = (lang: string, ns: string) => JSON.parse(readFileSync(new URL(`../../../locales/${lang}/${ns}.json`, import.meta.url), "utf8")) as Record<string, unknown>;
const known = (key: string) => {
  const [ns, ...rest] = key.split(".");
  return catalog("en", ns!)[rest.join(".")] !== undefined && catalog("es", ns!)[rest.join(".")] !== undefined;
};
const at = (ms: number) => ({ seconds: BigInt(Math.floor(ms / 1000)), nanos: (ms % 1000) * 1e6 }) as GachaBanner["startsAt"];
const banner = (extra: Partial<GachaBanner> = {}): GachaBanner =>
  ({
    id: "B",
    serverId: "S",
    name: "Stars",
    description: "",
    artUrl: "",
    currencyId: "C",
    pullPrice: 100n,
    multiPull: 10,
    multiPullPrice: 900n,
    tiers: [
      { id: "t1", name: "Legendary", rate: 30_000 },
      { id: "t2", name: "Common", rate: 970_000 },
    ],
    prizes: [
      { id: "p1", tierId: "t1", listingId: "L1", currencyId: "", amount: 0n, weight: 1, chance: 30_000 },
      { id: "p2", tierId: "t2", listingId: "", currencyId: "C", amount: 50n, weight: 3, chance: 727_500 },
      { id: "p3", tierId: "t2", listingId: "L2", currencyId: "", amount: 0n, weight: 1, chance: 242_500 },
    ],
    pityPulls: 50,
    pityTierId: "t1",
    duplicates: DUPLICATES_KEEP,
    refundCurrencyId: "",
    refundAmount: 0n,
    maxPulls: 0,
    available: true,
    position: 0,
    ...extra,
  }) as GachaBanner;
const progress = (pulls: number, sincePity: number, bannerId = "B") => ({ bannerId, pulls, sincePity }) as GachaProgress;

test("rates read and show as percentages", () => {
  assert.equal(percent(WHOLE), "100%");
  assert.equal(percent(30_000), "3%");
  assert.equal(percent(6_000), "0.6%");
  assert.equal(percent(125_000), "12.5%");
  assert.equal(percent(1), "0.0001%");
  assert.equal(parsePercent("3"), 30_000);
  assert.equal(parsePercent("0.6"), 6_000);
  assert.equal(parsePercent("12,5 %"), 125_000);
  assert.equal(parsePercent(".5"), 5_000);
  assert.equal(parsePercent("100"), WHOLE);
  for (const bad of ["", "abc", "100.5", "-1", "0.00001", "1e2"]) assert.equal(parsePercent(bad), null, bad);
  assert.equal(ratesTotal([{ rate: "3" }, { rate: "17" }, { rate: "80" }]), WHOLE);
  assert.equal(ratesTotal([{ rate: "3" }, { rate: "x" }]), null);
});

test("a banner's table lists every tier with its prizes, and tiers without a color take one by rarity", () => {
  const table = ratesTable(banner());
  assert.deepEqual(
    table.map((r) => [r.tier.id, r.prizes.map((p) => p.id)]),
    [
      ["t1", ["p1"]],
      ["t2", ["p2", "p3"]],
    ],
  );
  assert.equal(tierColor({ color: undefined }, 0), TIER_COLORS[0]);
  assert.equal(tierColor({ color: undefined }, 99), TIER_COLORS[TIER_COLORS.length - 1]);
  assert.equal(tierColor({ color: 0x123456 }, 0), 0x123456);
});

test("banners open and close on time, and members see only the ones they can pull on", () => {
  const now = 1_000_000_000_000;
  assert.deepEqual(openness(banner(), now), { state: "open", at: 0 });
  assert.deepEqual(openness(banner({ startsAt: at(now + 5000) }), now), { state: "soon", at: now + 5000 });
  assert.deepEqual(openness(banner({ endsAt: at(now) }), now), { state: "closed", at: now });
  assert.deepEqual(openness(banner({ endsAt: at(now + 1000) }), now), { state: "open", at: now + 1000 });
  const all = [banner({ id: "open" }), banner({ id: "hidden", available: false }), banner({ id: "gone", currencyId: "X" }), banner({ id: "closed", endsAt: at(now - 1) }), banner({ id: "soon", startsAt: at(now + 1) })];
  assert.deepEqual(
    shownBanners(all, ["C"], now).map((b) => b.id),
    ["open", "soon"],
  );
});

test("pulls cost the single price or the multi-pull's, pity counts down and limits run out", () => {
  assert.equal(pullCost(banner(), 1), 100);
  assert.equal(pullCost(banner(), 10), 900);
  assert.equal(pullCost(banner({ multiPullPrice: 0n }), 10), 1000);
  assert.equal(pullCost(banner(), 5), null);
  assert.equal(pullCost(banner({ multiPull: 0 }), 10), null);
  assert.equal(pityIn(banner(), undefined), 50);
  assert.equal(pityIn(banner(), progress(70, 20)), 30);
  assert.equal(pityIn(banner(), progress(70, 60)), 1);
  assert.equal(pityIn(banner({ pityPulls: 0 }), progress(1, 1)), null);
  assert.equal(pullsLeft(banner(), progress(5, 5)), null);
  assert.equal(pullsLeft(banner({ maxPulls: 10 }), progress(4, 4)), 6);
  assert.equal(pullsLeft(banner({ maxPulls: 10 }), progress(12, 0)), 0);
  assert.equal(pullsLeft(banner({ maxPulls: 10 }), undefined), 10);
});

test("a gacha-only listing knows its banners, and progress never goes back", () => {
  const banners = [banner({ id: "A" }), banner({ id: "B", prizes: [] })];
  assert.deepEqual(
    bannersWith(banners, "L1").map((b) => b.id),
    ["A"],
  );
  assert.deepEqual(bannersWith(banners, "nope"), []);
  const had = { B: progress(5, 5) };
  assert.equal(withProgress(had, undefined), had);
  assert.equal(withProgress(had, progress(3, 3)), had);
  assert.deepEqual(withProgress(had, progress(6, 0)).B, progress(6, 0));
  assert.deepEqual(Object.keys(withProgress(had, progress(1, 1, "C"))), ["B", "C"]);
});

test("every outcome, problem and line has words in both languages", () => {
  for (const o of [0, OUTCOME_WON, OUTCOME_DUPLICATE, OUTCOME_REFUNDED]) assert.ok(known(outcomeLabel(o)), String(o));
  for (const message of [
    "this banner isn't open yet",
    "this banner has closed",
    "you've made every pull this banner allows",
    "you have 3 pulls left on this banner",
    "nothing on this banner can drop for you right now",
    "this can only be won from the server's gacha banners",
  ]) {
    const problem = economyProblem({ code: 9, message });
    assert.ok(problem, message);
    assert.ok(known(problem.key), problem.key);
  }
  assert.deepEqual(economyProblem({ code: 9, message: "you have 3 pulls left on this banner" })?.values, { count: "3" });
});

test("a new banner starts with three tiers that add up to 100% and nothing to win", () => {
  const draft = bannerDraft(undefined, "C", 2, ["L", "R", "C"]);
  assert.deepEqual(
    draft.tiers.map((t) => t.name),
    ["L", "R", "C"],
  );
  assert.equal(ratesTotal(draft.tiers), WHOLE);
  assert.equal(draft.position, "2");
  assert.equal(draft.available, false);
  assert.deepEqual(bannerValue({ ...draft, name: "Stars" }), { problem: "prizes" });
  assert.equal(newTierId(draft.tiers), "t4");
  assert.equal(newTierId([{ id: "t2" }]), "t3");
  assert.equal(newTierId([{ id: "t2" }, { id: "t3" }]), "t4");
  assert.equal(draftChanged(undefined, draft), true);
});

test("a saved banner round-trips through the editor unchanged", () => {
  const saved = banner({ startsAt: at(Date.UTC(2026, 9, 14, 12, 30)), maxPulls: 30 });
  const draft = bannerDraft(saved);
  assert.equal(draftChanged(saved, draft), false);
  assert.equal(draftChanged(saved, { ...draft, name: "Moons" }), true);
  assert.equal(draft.artUrl, "");
  assert.equal(draftChanged(saved, { ...draft, artUrl: "https://fuwa.test/media/box" }), true);
  const boxed = bannerDraft({ ...saved, artUrl: "https://fuwa.test/media/box" });
  const sent = bannerValue(boxed, saved.id);
  assert.ok("banner" in sent && sent.banner.artUrl === "https://fuwa.test/media/box");
  const value = bannerValue(draft, saved.id);
  assert.ok("banner" in value);
  const b = value.banner;
  assert.equal(b.id, "B");
  assert.equal(b.pullPrice, 100n);
  assert.equal(b.multiPull, 10);
  assert.equal(b.multiPullPrice, 900n);
  assert.deepEqual(
    b.tiers.map((t) => [t.id, t.rate]),
    [
      ["t1", 30_000],
      ["t2", 970_000],
    ],
  );
  assert.deepEqual(b.prizes[1], { id: "p2", tierId: "t2", listingId: "", currencyId: "C", amount: 50n, weight: 3 });
  assert.deepEqual(b.prizes[0], { id: "p1", tierId: "t1", listingId: "L1", currencyId: "", amount: 0n, weight: 1 });
  assert.equal(b.pityPulls, 50);
  assert.equal(b.pityTierId, "t1");
  assert.equal(b.maxPulls, 30);
  assert.equal(b.startsAtMs, Date.UTC(2026, 9, 14, 12, 30));
  assert.equal(b.endsAtMs, null);
});

test("the editor shows each prize's chance and sends new prizes without an id", () => {
  const draft = bannerDraft(banner());
  const chances = draftChances(draft);
  assert.deepEqual(chances, { p1: 30_000, p2: 727_500, p3: 242_500 });
  const added: BannerDraft = { ...draft, prizes: [...draft.prizes, { id: "new:1", tierId: "t1", listingId: "", currencyId: "C", amount: "500", weight: "1" }] };
  assert.equal(draftChances(added)["new:1"], 15_000);
  assert.equal(draftChances({ ...added, prizes: added.prizes.map((p) => (p.id === "p1" ? { ...p, weight: "x" } : p)) })["new:1"], 30_000);
  const value = bannerValue(added);
  assert.ok("banner" in value);
  assert.equal(value.banner.prizes.at(-1)!.id, "");
  assert.equal(value.banner.id, "");
});

test("the editor names the first thing wrong with a banner", () => {
  const good = bannerDraft(banner());
  const wrong = (patch: Partial<BannerDraft>) => {
    const out = bannerValue({ ...good, ...patch });
    return "problem" in out ? [out.problem, out.index] : null;
  };
  assert.deepEqual(wrong({ name: "  " }), ["name", undefined]);
  assert.deepEqual(wrong({ currencyId: "" }), ["currency", undefined]);
  assert.deepEqual(wrong({ price: "0" }), ["price", undefined]);
  assert.deepEqual(wrong({ multi: "1" }), ["multi", undefined]);
  assert.equal(wrong({ multi: "" }), null);
  assert.deepEqual(wrong({ multiPrice: "abc" }), ["multiPrice", undefined]);
  assert.deepEqual(wrong({ tiers: [] }), ["tiers", undefined]);
  assert.deepEqual(wrong({ tiers: good.tiers.map((t, i) => (i === 1 ? { ...t, name: "" } : t)) }), ["tierName", 1]);
  assert.deepEqual(wrong({ tiers: good.tiers.map((t, i) => (i === 0 ? { ...t, rate: "lots" } : t)) }), ["tierRate", 0]);
  assert.deepEqual(wrong({ tiers: good.tiers.map((t, i) => (i === 0 ? { ...t, rate: "4" } : t)) }), ["ratesTotal", undefined]);
  assert.deepEqual(wrong({ prizes: good.prizes.map((p, i) => (i === 2 ? { ...p, weight: "0" } : p)) }), ["prizeWeight", 2]);
  assert.deepEqual(wrong({ prizes: good.prizes.map((p, i) => (i === 1 ? { ...p, amount: "" } : p)) }), ["prizeAmount", 1]);
  assert.deepEqual(wrong({ prizes: good.prizes.map((p, i) => (i === 0 ? { ...p, listingId: "", currencyId: "" } : p)) }), ["prizeWhat", 0]);
  assert.deepEqual(wrong({ prizes: good.prizes.filter((p) => p.tierId !== "t1") }), ["emptyTier", 0]);
  // A tier at 0% may stay empty.
  assert.equal(wrong({ tiers: [...good.tiers, { id: "t3", name: "Never", color: null, rate: "0" }] }), null);
  assert.deepEqual(wrong({ pityPulls: "0" }), ["pity", undefined]);
  assert.deepEqual(wrong({ pityTierId: "gone" }), ["pity", undefined]);
  assert.equal(wrong({ pityPulls: "", pityTierId: "gone" }), null);
  assert.deepEqual(wrong({ duplicates: DUPLICATES_REFUND, refundCurrencyId: "C", refundAmount: "" }), ["refund", undefined]);
  assert.equal(wrong({ duplicates: DUPLICATES_REFUND, refundCurrencyId: "C", refundAmount: "25" }), null);
  assert.deepEqual(wrong({ maxPulls: "-1" }), ["maxPulls", undefined]);
  assert.deepEqual(wrong({ startsAt: "2026-10-14T12:00", endsAt: "2026-10-14T11:00" }), ["dates", undefined]);
  assert.deepEqual(wrong({ position: "first" }), ["position", undefined]);
});

test("datetime-local inputs read back to the same moment", () => {
  const moment = new Date(2026, 9, 14, 18, 45).getTime();
  assert.equal(localInput(moment), "2026-10-14T18:45");
  assert.equal(fromLocalInput(localInput(moment)), moment);
  assert.equal(localInput(0), "");
  assert.equal(fromLocalInput(""), null);
  assert.equal(fromLocalInput("not a date"), null);
});

test("prize pickers list gacha-only listings first", () => {
  const shop = [
    { id: "a", gachaOnly: false, position: 0 },
    { id: "b", gachaOnly: true, position: 5 },
    { id: "c", gachaOnly: true, position: 1 },
  ] as ShopListing[];
  assert.deepEqual(
    prizeListings(shop).map((l) => l.id),
    ["c", "b", "a"],
  );
});

test("the reveal ranks tiers, gives them stars and plays to one clock", () => {
  const banner = { tiers: ["legendary", "epic", "rare", "common"].map((id) => ({ id })), pityTierId: "epic" } as unknown as GachaBanner;
  assert.deepEqual(
    banner.tiers.map((t) => rarity(banner, t.id)),
    [3, 2, 1, 0],
  );
  assert.deepEqual(
    banner.tiers.map((t) => stars(banner, t.id)),
    [4, 3, 2, 1],
  );
  assert.equal(rarity({ ...banner, pityTierId: "" }, "epic"), 1);
  assert.equal(rarity(banner, "gone"), 0);
  assert.equal(rarity({ tiers: [{ id: "only" }], pityTierId: "" } as unknown as GachaBanner, "only"), 1);
  assert.equal(bestTier(banner, [{ tierId: "common" }, { tierId: "epic" }, { tierId: "rare" }]), 1);
  assert.equal(bestTier(banner, []), 3);
  // The rare card waits a beat before it turns and lingers after.
  assert.deepEqual(revealPlan([0, 3, 0], false), { burst: 1300, deal: 1650, flips: [2150, 2750, 3290], done: 3510 });
  assert.deepEqual(revealPlan([0, 3], true), { burst: 0, deal: 0, flips: [0, 0], done: 0 });
});
