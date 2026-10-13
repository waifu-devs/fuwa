import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { Balance, Currency, EarnRule, LedgerEntry, ShopListing } from "@/gen/fuwa/v1/types_pb";
import {
  ACTIONS,
  actionLabel,
  applyEconomy,
  chartColumns,
  currencyChange,
  currencyDraft,
  economyProblem,
  DAILY,
  dailyOf,
  EMPTY,
  forSale,
  formRules,
  ledgerLine,
  LEDGER_EARN,
  LEDGER_GRANT,
  LEDGER_KINDS,
  LEDGER_PURCHASE,
  LEDGER_TRANSFER,
  listingChange,
  listingDraft,
  LISTING_CUSTOM,
  LISTING_PROFILE_ITEM,
  LISTING_ROLE,
  lockedItems,
  MESSAGE,
  newWallet,
  niceMax,
  owns,
  parseAmount,
  rulesForm,
  share,
  stockLeft,
  utcDay,
  withBalance,
  withPage,
  type EconomyPayload,
  type ServerEconomy,
} from "./economy.ts";

const catalog = (lang: string, ns: string) => JSON.parse(readFileSync(new URL(`../../../locales/${lang}/${ns}.json`, import.meta.url), "utf8")) as Record<string, unknown>;
const en = { chattools: catalog("en", "chattools"), serversettings: catalog("en", "serversettings") };
const es = { chattools: catalog("es", "chattools"), serversettings: catalog("es", "serversettings") };
const known = (key: string) => {
  const [ns, ...rest] = key.split(".") as ["chattools" | "serversettings", ...string[]];
  const short = rest.join(".");
  return en[ns]?.[short] !== undefined && es[ns]?.[short] !== undefined;
};

const DAY = 86_400_000;
const at = (ms: number) => ({ seconds: BigInt(Math.floor(ms / 1000)), nanos: 0 }) as Balance["dailyClaimedAt"];
const cur = (id: string, position = 0, extra: Partial<Currency> = {}): Currency => ({ id, position, serverId: "S", name: id, iconEmoji: "", iconEmojiId: "", description: "", earn: [], transferable: false, ...extra }) as Currency;
const bal = (currencyId: string, amount: number, extra: Partial<Balance> = {}): Balance => ({ currencyId, userId: "me", amount: BigInt(amount), earned: 0n, spent: 0n, dailyStreak: 0, ...extra }) as Balance;
const lst = (id: string, extra: Partial<ShopListing> = {}): ShopListing =>
  ({ id, currencyId: "C", kind: LISTING_CUSTOM, targetId: "", name: id, description: "", price: 10n, sold: 0n, position: 0, available: true, ...extra }) as ShopListing;
const rule = (extra: Partial<EarnRule> = {}): EarnRule => ({ action: DAILY, amount: 50n, cooldownSeconds: 0, dailyCap: 0n, streakBonus: 10n, streakMaxDays: 3, ...extra }) as EarnRule;
const ev = (payload: EconomyPayload) => payload;

test("every action and ledger kind has words in both languages", () => {
  for (const a of ACTIONS) for (const key of [a.label, a.about]) assert.ok(known(key), key);
  for (const k of LEDGER_KINDS) assert.ok(known(k.label), k.label);
  assert.ok(known(actionLabel(99)));
  for (const kind of [LEDGER_EARN, LEDGER_GRANT, LEDGER_TRANSFER, LEDGER_PURCHASE, 9])
    for (const amount of [5n, -5n])
      for (const extra of [{ roleId: "" , actorId: "A" }, { roleId: "R", actorId: "A" }, { roleId: "", actorId: "" }])
        for (const action of [0, 1, 2, 3, 4, 5, 6]) {
          const key = ledgerLine({ kind, amount, action, ...extra } as LedgerEntry);
          assert.ok(known(key), key);
        }
});

test("ledger lines read by kind, direction and action", () => {
  assert.equal(ledgerLine({ kind: LEDGER_EARN, amount: 1n, action: DAILY } as LedgerEntry), "chattools.economy.line.earnDaily");
  assert.equal(ledgerLine({ kind: LEDGER_TRANSFER, amount: -3n } as LedgerEntry), "chattools.economy.line.sent");
  assert.equal(ledgerLine({ kind: LEDGER_TRANSFER, amount: 3n } as LedgerEntry), "chattools.economy.line.received");
  assert.equal(ledgerLine({ kind: LEDGER_GRANT, amount: -3n, roleId: "R", actorId: "A" } as LedgerEntry), "chattools.economy.line.takenRole");
  assert.equal(ledgerLine({ kind: LEDGER_GRANT, amount: 3n, roleId: "", actorId: "A" } as LedgerEntry), "chattools.economy.line.given");
  assert.equal(ledgerLine({ kind: LEDGER_PURCHASE, amount: -10n } as LedgerEntry), "chattools.economy.line.bought");
});

test("amounts as typed: whole numbers in range, separators let go", () => {
  assert.equal(parseAmount("1,000", 1, 5000), 1000);
  assert.equal(parseAmount(" 42 ", 1, 100), 42);
  assert.equal(parseAmount("-5", -10, 10), -5);
  assert.equal(parseAmount("0", 1, 10), null);
  assert.equal(parseAmount("1.5", 0, 10), null);
  assert.equal(parseAmount("abc", 0, 10), null);
  assert.equal(parseAmount("", 0, 10), null);
  assert.equal(parseAmount("11", 0, 10), null);
});

test("the daily reward: once per UTC day, streaks counted and capped", () => {
  const today = 20_000 * DAY + 5 * 3_600_000;
  // Never claimed: ready, the plain amount.
  assert.deepEqual(dailyOf(undefined, rule(), today), { ready: true, nextAt: 20_001 * DAY, streak: 0, reward: 50 });
  // Claimed today: not ready until UTC midnight; the streak still counts.
  const claimedToday = bal("C", 0, { dailyClaimedAt: at(20_000 * DAY + 1000), dailyStreak: 2 });
  const d = dailyOf(claimedToday, rule(), today);
  assert.equal(d.ready, false);
  assert.equal(d.nextAt, 20_001 * DAY);
  assert.equal(d.streak, 2);
  // Claimed yesterday: ready, one more day in a row (third day: two days of bonus).
  const yesterday = bal("C", 0, { dailyClaimedAt: at(19_999 * DAY + 80_000_000), dailyStreak: 2 });
  assert.deepEqual(dailyOf(yesterday, rule(), today), { ready: true, nextAt: 20_001 * DAY, streak: 2, reward: 70 });
  // The bonus stops at streak_max_days.
  const long = bal("C", 0, { dailyClaimedAt: at(19_999 * DAY), dailyStreak: 30 });
  assert.equal(dailyOf(long, rule(), today).reward, 50 + 10 * 3);
  // A missed day starts over.
  const missed = bal("C", 0, { dailyClaimedAt: at(19_998 * DAY), dailyStreak: 30 });
  assert.equal(dailyOf(missed, rule(), today).streak, 0);
  assert.equal(dailyOf(missed, rule(), today).reward, 50);
  // No streak bonus without max days.
  assert.equal(dailyOf(yesterday, rule({ streakMaxDays: 0 }), today).reward, 50);
  assert.equal(utcDay(DAY - 1), 0);
  assert.equal(utcDay(DAY), 1);
});

test("earn rules: form and back, with every problem found", () => {
  const earn = [rule(), rule({ action: MESSAGE, amount: 2n, cooldownSeconds: 60, streakBonus: 0n, streakMaxDays: 0 })];
  const form = rulesForm(earn);
  assert.equal(form[DAILY]!.on, true);
  assert.equal(form[DAILY]!.streakBonus, "10");
  assert.equal(form[MESSAGE]!.cooldown, "60");
  assert.equal(form[3]!.on, false);
  const back = formRules(form);
  assert.ok("rules" in back);
  assert.equal(back.rules.length, 2);
  assert.deepEqual(
    back.rules.find((r) => r.action === MESSAGE),
    { action: MESSAGE, amount: 2n, cooldownSeconds: 60, dailyCap: 0n, streakBonus: 0n, streakMaxDays: 0 },
  );
  assert.deepEqual(formRules({ ...form, [MESSAGE]: { ...form[MESSAGE]!, amount: "0" } }), { problem: { action: MESSAGE, field: "amount" } });
  assert.deepEqual(formRules({ ...form, [MESSAGE]: { ...form[MESSAGE]!, cooldown: "999999" } }), { problem: { action: MESSAGE, field: "cooldown" } });
  // Fields an action doesn't have are ignored.
  const join = { ...form, 3: { on: true, amount: "5", cooldown: "x", dailyCap: "", streakBonus: "y", streakMaxDays: "" } };
  assert.ok("rules" in formRules(join));
});

test("a currency's change says only what changed, and counts it", () => {
  const saved = cur("C", 0, { name: "Coins", iconEmoji: "🪙", earn: [rule()] });
  const draft = currencyDraft(saved);
  const same = currencyChange(saved, draft);
  assert.ok("change" in same && same.count === 0);
  const renamed = currencyChange(saved, { ...draft, name: " Gems ", transferable: true });
  assert.ok("change" in renamed);
  assert.equal(renamed.count, 2);
  assert.equal(renamed.change.name, "Gems");
  assert.equal(renamed.change.transferable, true);
  assert.equal(renamed.change.replaceEarn, false);
  const rules = currencyChange(saved, { ...draft, rules: { ...draft.rules, [DAILY]: { ...draft.rules[DAILY]!, amount: "60" } } });
  assert.ok("change" in rules && rules.change.replaceEarn && rules.change.earn[0]!.amount === 60n);
  assert.deepEqual(currencyChange(saved, { ...draft, name: "  " }), { problem: "name" });
  const fresh = currencyChange(undefined, { ...currencyDraft(undefined), name: "Petals" });
  assert.ok("change" in fresh && fresh.change.replaceEarn && fresh.change.iconEmoji === "🪙");
});

test("a listing's change: create, edit, stock to unlimited", () => {
  const draft = { ...listingDraft(undefined, "C", 3), kind: LISTING_CUSTOM, name: "A hug" };
  const made = listingChange(undefined, draft);
  assert.ok("create" in made);
  assert.equal(made.create.position, 3);
  assert.equal(made.create.stock, undefined);
  assert.deepEqual(listingChange(undefined, { ...draft, name: "" }), { problem: "name" });
  assert.deepEqual(listingChange(undefined, { ...draft, kind: LISTING_ROLE }), { problem: "target" });
  assert.deepEqual(listingChange(undefined, { ...draft, price: "0" }), { problem: "price" });
  const saved = lst("L", { stock: 5n, name: "A hug" });
  const edit = listingChange(saved, { ...listingDraft(saved), stock: "" });
  assert.ok("change" in edit && edit.count === 1 && edit.change.unlimited);
  const restock = listingChange(saved, { ...listingDraft(saved), stock: "9", price: "12" });
  assert.ok("change" in restock && restock.count === 2 && restock.change.stock === 9n && restock.change.price === 12n);
});

test("the shop: what's for sale, what you own, what's locked", () => {
  const shop = [lst("a"), lst("b", { available: false }), lst("c", { currencyId: "gone" })];
  assert.deepEqual(
    forSale(shop, [cur("C")]).map((l) => l.id),
    ["a"],
  );
  assert.equal(stockLeft(lst("x")), null);
  assert.equal(stockLeft(lst("x", { stock: 0n })), 0);
  assert.ok(owns(lst("r", { kind: LISTING_ROLE, targetId: "R" }), ["R"], []));
  assert.ok(owns(lst("i", { kind: LISTING_PROFILE_ITEM, targetId: "I" }), [], ["I"]));
  assert.ok(!owns(lst("c"), [], []));
  const items = [
    lst("i1", { kind: LISTING_PROFILE_ITEM, targetId: "I", price: 30n }),
    lst("i2", { kind: LISTING_PROFILE_ITEM, targetId: "I", price: 20n }),
    lst("j", { kind: LISTING_PROFILE_ITEM, targetId: "J", available: false }),
    lst("k", { kind: LISTING_PROFILE_ITEM, targetId: "K" }),
  ];
  const locked = lockedItems(items, ["K"]);
  assert.deepEqual(locked.get("I"), { price: 20, currencyId: "C" });
  assert.equal(locked.get("J"), null);
  assert.equal(locked.has("K"), false);
});

test("events keep a server's economy, and applying one twice changes nothing", () => {
  const start: ServerEconomy = { currencies: [cur("A")], shop: [lst("L", { currencyId: "A" })], wallet: newWallet([bal("A", 5)], []) };
  // A new currency starts at zero in the wallet; a deleted one takes its balance and listings.
  const listed = ev({ case: "currenciesUpdated", value: { currencies: [cur("B", 1), cur("A2", 0)] } } as EconomyPayload);
  const once = applyEconomy(start, listed, "me").state;
  assert.deepEqual(
    once.currencies!.map((c) => c.id),
    ["A2", "B"],
  );
  assert.deepEqual(
    once.wallet!.balances.map((b) => [b.currencyId, Number(b.amount)]),
    [
      ["A2", 0],
      ["B", 0],
    ],
  );
  assert.deepEqual(once.shop, []);
  assert.deepEqual(applyEconomy(once, listed, "me").state, once);

  // A balance: replaced, mine only, never put back by an older entry.
  const newer = ev({ case: "balanceUpdated", value: { balance: bal("A", 9), entry: { id: "02" } } } as EconomyPayload);
  const older = ev({ case: "balanceUpdated", value: { balance: bal("A", 7), entry: { id: "01" } } } as EconomyPayload);
  let s = applyEconomy(start, newer, "me").state;
  s = applyEconomy(s, older, "me").state;
  s = applyEconomy(s, newer, "me").state;
  assert.equal(Number(s.wallet!.balances[0]!.amount), 9);
  const someone = ev({ case: "balanceUpdated", value: { balance: bal("A", 1, { userId: "them" }), entry: { id: "03" } } } as EconomyPayload);
  assert.equal(applyEconomy(s, someone, "me").state, s);

  // A listing bought: replaced in place.
  const sold = ev({ case: "shopListingUpdated", value: { listing: lst("L", { currencyId: "A", sold: 1n }) } } as EconomyPayload);
  const after = applyEconomy(start, sold, "me").state;
  assert.equal(after.shop!.length, 1);
  assert.equal(Number(after.shop![0]!.sold), 1);
  assert.deepEqual(applyEconomy(after, sold, "me").state.shop, after.shop);
  // Nothing to patch before the shop loads.
  assert.equal(applyEconomy(EMPTY, sold, "me").state, EMPTY);

  // An item bought by me: owned, once.
  const purchase = ev({ case: "purchaseMade", value: { entry: { userId: "me" }, listing: lst("P", { kind: LISTING_PROFILE_ITEM, targetId: "I", currencyId: "A" }) } } as EconomyPayload);
  const owned = applyEconomy(applyEconomy(start, purchase, "me").state, purchase, "me").state;
  assert.deepEqual(owned.wallet!.ownedItemIds, ["I"]);
  const theirs = ev({ case: "purchaseMade", value: { entry: { userId: "them" }, listing: lst("P", { kind: LISTING_PROFILE_ITEM, targetId: "I" }) } } as EconomyPayload);
  assert.deepEqual(applyEconomy(start, theirs, "me").state.wallet!.ownedItemIds, []);

  // Everyone's wallets moved: read again, only when one is held.
  const moved = ev({ case: "walletsChanged", value: { currencyId: "A" } } as EconomyPayload);
  assert.equal(applyEconomy(start, moved, "me").reread, true);
  assert.equal(applyEconomy(EMPTY, moved, "me").reread, false);
  // A first currency: the wallet is read.
  assert.equal(applyEconomy(EMPTY, listed, "me").reread, true);
  assert.equal(applyEconomy(once, listed, "me").reread, false);
});

test("a wallet's balances from answers and events agree", () => {
  const w = newWallet([bal("A", 1)], []);
  const w2 = withBalance(w, bal("A", 3), "05");
  assert.equal(Number(w2.balances[0]!.amount), 3);
  assert.equal(withBalance(w2, bal("A", 2), "04"), w2);
  assert.equal(withBalance(w2, bal("B", 2)).balances.length, 2);
});

test("ledger pages join newest first, each entry once", () => {
  assert.deepEqual(
    withPage([{ id: "05" }, { id: "04" }], [{ id: "04" }, { id: "02" }, { id: "03" }]).map((e) => e.id),
    ["05", "04", "03", "02"],
  );
});

test("the chart's scale and bars", () => {
  assert.equal(niceMax(0), 1);
  assert.equal(niceMax(7), 10);
  assert.equal(niceMax(12), 20);
  assert.equal(niceMax(450), 500);
  assert.equal(niceMax(1000), 1000);
  const { max, columns } = chartColumns([
    { earned: 30n, granted: 0n, spent: 10n },
    { earned: 0n, granted: 40n, spent: 0n },
  ]);
  assert.equal(max, 50);
  assert.deepEqual(columns[1], { earned: 0, granted: 0.8, spent: 0 });
  assert.equal(share(5, 0), 0);
  assert.equal(share(5, 10), 0.5);
  assert.equal(share(15, 10), 1);
});

test("the server's refusals are worded by the app, and unknown ones aren't", () => {
  const pre = (message: string) => economyProblem({ code: 9, message });
  assert.deepEqual(pre("you don't have enough Coins"), { key: "chattools.economy.notEnough", values: { currency: "Coins" } });
  assert.equal(pre("the price changed: have another look before buying")?.key, "chattools.economy.problem.priceChanged");
  assert.equal(pre("it's sold out")?.key, "chattools.economy.problem.soldOut");
  assert.equal(pre("you've claimed today's reward already; come back tomorrow")?.key, "chattools.economy.problem.claimed");
  assert.deepEqual(pre("Gems can't be sent between members"), { key: "chattools.economy.problem.notTransferable", values: { currency: "Gems" } });
  assert.deepEqual(pre("Gems has no daily reward"), { key: "chattools.economy.problem.noDaily", values: { currency: "Gems" } });
  assert.equal(pre("you can't hold any more")?.key, "chattools.economy.problem.full");
  assert.equal(pre("this item is sold in the server's shop: buy it first")?.key, "chattools.economy.problem.buyFirst");
  assert.equal(pre("a role in the shop can't carry permissions that moderate or manage the server")?.key, "chattools.economy.problem.powerfulRole");
  assert.equal(economyProblem({ code: 6, message: "you have this role already" })?.key, "chattools.economy.problem.haveRole");
  assert.equal(economyProblem({ code: 6, message: "you have this item already" })?.key, "chattools.economy.problem.haveItem");
  assert.equal(pre("something new"), null);
  assert.equal(economyProblem({ code: 3, message: "you don't have enough Coins" }), null);
  const all = [
    "you don't have enough X",
    "the price changed",
    "it's sold out",
    "claimed today's reward already",
    "X can't be sent between members",
    "X has no daily reward",
    "can't hold any more",
    "sold in the server's shop",
    "can't carry permissions",
  ].map((m) => pre(m)!);
  all.push(economyProblem({ code: 6, message: "role" })!, economyProblem({ code: 6, message: "item" })!);
  for (const worded of all) assert.ok(known(worded.key), worded.key);
});
