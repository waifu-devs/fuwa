import type { Balance, Currency, EarnRule, Event, LedgerEntry, ShopListing } from "@/gen/fuwa/v1/types_pb";
import type { Key } from "../i18n/i18n.ts";

/*
 * A server's economy (docs/economy.md): the small decisions its screens
 * share, from keeping a server's currencies, shop and wallet in step with
 * the events that change them to when the next daily reward is due. Only
 * type imports, so node's test runner reads this file as is; the enums it
 * needs are written out below, as the protocol numbers them.
 */

/** fuwa.v1.CurrencyAction. */
export const MESSAGE = 1;
export const DAILY = 2;
export const JOIN = 3;
export const ONBOARDING = 4;
export const REACTION_RECEIVED = 5;
export const VOICE_MINUTE = 6;
export const GAME_MINUTE = 7;

/** fuwa.v1.ShopListingKind. */
export const LISTING_ROLE = 1;
export const LISTING_PROFILE_ITEM = 2;
export const LISTING_CUSTOM = 3;

/** fuwa.v1.LedgerKind. */
export const LEDGER_EARN = 1;
export const LEDGER_GRANT = 2;
export const LEDGER_TRANSFER = 3;
export const LEDGER_PURCHASE = 4;

/** What a currency without an icon shows, as every app does. */
export const FALLBACK_ICON = "🪙";

/** As the server takes them. */
export const NAME_MAX = 32;
export const DESCRIPTION_MAX = 200;
export const LISTING_NAME_MAX = 60;
export const NOTE_MAX = 200;
export const AMOUNT_MAX = 1_000_000_000;
export const PRICE_MAX = 1_000_000_000_000;
export const COOLDOWN_MAX = 86_400;
/** Games one GAME_MINUTE rule names at most, and how long a name is. */
export const GAMES_MAX = 50;
export const GAME_NAME_MAX = 128;
/** A page of the ledger. */
export const LEDGER_PAGE = 50;
/** The finance page's periods, in days. */
export const PERIODS = [7, 30, 90, 365] as const;

const DAY_MS = 86_400_000;

/** Each action a currency can reward, in the order the editor lists them, with its words and what else its rule takes. */
export const ACTIONS: { action: number; label: Key; about: Key; cooldown?: true; streak?: true; games?: true; feature?: string }[] = [
  { action: MESSAGE, label: "serversettings.economy.action.message", about: "serversettings.economy.action.messageAbout", cooldown: true },
  { action: REACTION_RECEIVED, label: "serversettings.economy.action.reaction", about: "serversettings.economy.action.reactionAbout", cooldown: true },
  { action: VOICE_MINUTE, label: "serversettings.economy.action.voice", about: "serversettings.economy.action.voiceAbout" },
  { action: GAME_MINUTE, label: "serversettings.economy.action.game", about: "serversettings.economy.action.gameAbout", games: true, feature: "economy-games" },
  { action: DAILY, label: "serversettings.economy.action.daily", about: "serversettings.economy.action.dailyAbout", streak: true },
  { action: JOIN, label: "serversettings.economy.action.join", about: "serversettings.economy.action.joinAbout" },
  { action: ONBOARDING, label: "serversettings.economy.action.onboarding", about: "serversettings.economy.action.onboardingAbout" },
];

/** An action's name; one this app doesn't know reads as "Something else". */
export const actionLabel = (action: number): Key => ACTIONS.find((a) => a.action === action)?.label ?? "serversettings.economy.action.other";

/** Ledger kinds, as the finance page filters them. */
export const LEDGER_KINDS: { kind: number; label: Key }[] = [
  { kind: LEDGER_EARN, label: "chattools.economy.kind.earn" },
  { kind: LEDGER_GRANT, label: "chattools.economy.kind.grant" },
  { kind: LEDGER_TRANSFER, label: "chattools.economy.kind.transfer" },
  { kind: LEDGER_PURCHASE, label: "chattools.economy.kind.purchase" },
];

// ── Numbers ──────────────────────────────────────────────────────────────

/** An int64 from the wire as a number (amounts stay far below 2^53). */
export const num = (v: bigint | number | undefined): number => Number(v ?? 0);

/** A timestamp from the wire in unix milliseconds, 0 when unset. */
export const ms = (at: { seconds: bigint; nanos: number } | undefined): number => (at ? Number(at.seconds) * 1000 + Math.floor(at.nanos / 1e6) : 0);

/**
 * A whole number someone typed, between `min` and `max`, or null when it
 * isn't one. Spaces and the thousands separators people type are let go.
 */
export function parseAmount(text: string, min: number, max: number): number | null {
  const plain = text.replace(/[\s,_']/g, "");
  if (!/^-?\d+$/.test(plain)) return null;
  const n = Number(plain);
  return Number.isSafeInteger(n) && n >= min && n <= max ? n : null;
}

// ── Lists in order ───────────────────────────────────────────────────────

const byPosition = <T extends { position: number; id: string }>(a: T, b: T) => a.position - b.position || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);

export const sortCurrencies = <T extends Pick<Currency, "position" | "id">>(list: readonly T[]): T[] => [...list].sort(byPosition);
export const sortListings = <T extends Pick<ShopListing, "position" | "id">>(list: readonly T[]): T[] => [...list].sort(byPosition);

/** What members see of the shop: the available listings, of currencies still there. */
export const forSale = <T extends Pick<ShopListing, "available" | "currencyId">>(listings: readonly T[], currencies: readonly Pick<Currency, "id">[]): T[] =>
  listings.filter((l) => l.available && currencies.some((c) => c.id === l.currencyId));

/** How many are left: null for no limit. */
export const stockLeft = (listing: Pick<ShopListing, "stock">): number | null => (listing.stock === undefined ? null : num(listing.stock));

/** Whether you already have what a listing sells: a role you hold, an item you bought. Custom ones are never "owned". */
export function owns(listing: Pick<ShopListing, "kind" | "targetId">, roleIds: readonly string[], ownedItemIds: readonly string[]): boolean {
  if (listing.kind === LISTING_ROLE) return roleIds.includes(listing.targetId);
  if (listing.kind === LISTING_PROFILE_ITEM) return ownedItemIds.includes(listing.targetId);
  return false;
}

/**
 * Profile items that are for sale (any listing, available or not, keeps
 * them for buyers) and not yours: what the profile picker locks, with the
 * cheapest available listing's price to show.
 */
export function lockedItems(
  listings: readonly Pick<ShopListing, "kind" | "targetId" | "available" | "price" | "currencyId">[],
  ownedItemIds: readonly string[],
): Map<string, { price: number; currencyId: string } | null> {
  const locked = new Map<string, { price: number; currencyId: string } | null>();
  for (const l of listings) {
    if (l.kind !== LISTING_PROFILE_ITEM || ownedItemIds.includes(l.targetId)) continue;
    const had = locked.get(l.targetId);
    if (!l.available) {
      if (had === undefined) locked.set(l.targetId, null);
      continue;
    }
    if (!had || num(l.price) < had.price) locked.set(l.targetId, { price: num(l.price), currencyId: l.currencyId });
  }
  return locked;
}

// ── The state an app keeps for a server ──────────────────────────────────

export type Wallet = {
  balances: Balance[];
  ownedItemIds: string[];
  /** The newest ledger entry applied per currency, so an older answer or event never puts a balance back. */
  seen: Record<string, string>;
};

/** One server's economy as the app knows it; null for what isn't loaded yet. */
export type ServerEconomy = { currencies: Currency[] | null; shop: ShopListing[] | null; wallet: Wallet | null };

export const EMPTY: ServerEconomy = { currencies: null, shop: null, wallet: null };

export const newWallet = (balances: readonly Balance[], ownedItemIds: readonly string[]): Wallet => ({ balances: [...balances], ownedItemIds: [...ownedItemIds], seen: {} });

/**
 * A wallet with one balance as it is after `entryId` (a claim, a send, a
 * purchase, or its event). Applying it twice changes nothing, and one older
 * than what's there is let go.
 */
export function withBalance(wallet: Wallet, balance: Balance, entryId = ""): Wallet {
  const seen = wallet.seen[balance.currencyId] ?? "";
  if (entryId && seen && entryId < seen) return wallet;
  const at = wallet.balances.findIndex((b) => b.currencyId === balance.currencyId);
  const balances = at === -1 ? [...wallet.balances, balance] : wallet.balances.map((b, n) => (n === at ? balance : b));
  return { ...wallet, balances, seen: entryId && entryId > seen ? { ...wallet.seen, [balance.currencyId]: entryId } : wallet.seen };
}

/** A wallet with an item bought (once). */
export const withOwned = (wallet: Wallet, itemId: string): Wallet =>
  wallet.ownedItemIds.includes(itemId) ? wallet : { ...wallet, ownedItemIds: [...wallet.ownedItemIds, itemId] };

/** A shop with one listing as it is now, put in its place. */
export function withListing(shop: readonly ShopListing[], listing: ShopListing): ShopListing[] {
  const at = shop.findIndex((l) => l.id === listing.id);
  return sortListings(at === -1 ? [...shop, listing] : shop.map((l, n) => (n === at ? listing : l)));
}

export type EconomyPayload = Extract<
  Event["payload"],
  { case: "currenciesUpdated" | "shopUpdated" | "shopListingUpdated" | "balanceUpdated" | "walletsChanged" | "purchaseMade" }
>;

export const ECONOMY_EVENTS: ReadonlySet<string> = new Set(["currenciesUpdated", "shopUpdated", "shopListingUpdated", "balanceUpdated", "walletsChanged", "purchaseMade"]);

/**
 * A server's economy after one of its events, and whether the wallet must
 * be read (everyone's balances moved at once, or there's a first
 * currency). Applying an event twice gives the same state. `me` is the
 * account the app is signed in as.
 */
export function applyEconomy(state: ServerEconomy, payload: EconomyPayload, me: string): { state: ServerEconomy; reread: boolean } {
  switch (payload.case) {
    case "currenciesUpdated": {
      const currencies = sortCurrencies(payload.value.currencies);
      const ids = new Set(currencies.map((c) => c.id));
      // A deleted currency takes its balances and listings along; a new one starts at zero.
      let wallet = state.wallet;
      if (wallet) {
        const kept = wallet.balances.filter((b) => ids.has(b.currencyId));
        const fresh = currencies
          .filter((c) => !kept.some((b) => b.currencyId === c.id))
          .map((c) => ({ currencyId: c.id, userId: me, amount: 0n, earned: 0n, spent: 0n, dailyStreak: 0 }) as unknown as Balance);
        wallet = kept.length === wallet.balances.length && !fresh.length ? wallet : { ...wallet, balances: [...kept, ...fresh] };
      }
      const shop = state.shop && state.shop.some((l) => !ids.has(l.currencyId)) ? state.shop.filter((l) => ids.has(l.currencyId)) : state.shop;
      // A first currency: there's a wallet to read now.
      return { state: { currencies, shop, wallet }, reread: !wallet && currencies.length > 0 };
    }
    case "shopUpdated":
      return { state: { ...state, shop: sortListings(payload.value.listings) }, reread: false };
    case "shopListingUpdated": {
      const listing = payload.value.listing;
      if (!listing || !state.shop) return { state, reread: false };
      return { state: { ...state, shop: withListing(state.shop, listing) }, reread: false };
    }
    case "balanceUpdated": {
      const { balance, entry } = payload.value;
      if (!balance || !state.wallet || (balance.userId && balance.userId !== me)) return { state, reread: false };
      return { state: { ...state, wallet: withBalance(state.wallet, balance, entry?.id) }, reread: false };
    }
    case "walletsChanged":
      return { state, reread: !!state.wallet };
    case "purchaseMade": {
      const { entry, listing } = payload.value;
      let next = state;
      if (listing && state.shop) next = { ...next, shop: withListing(state.shop, listing) };
      if (entry?.userId === me && listing?.kind === LISTING_PROFILE_ITEM && next.wallet) next = { ...next, wallet: withOwned(next.wallet, listing.targetId) };
      return { state: next, reread: false };
    }
  }
}

// ── The daily reward ─────────────────────────────────────────────────────

/** The UTC day a moment falls on, counted from 1970. */
export const utcDay = (at: number) => Math.floor(at / DAY_MS);

export type Daily = {
  /** Whether Claim may be pressed now. */
  ready: boolean;
  /** When it may be next (the coming UTC midnight), when it can't now. */
  nextAt: number;
  /** Days in a row so far, still counting (claimed today or yesterday), else 0. */
  streak: number;
  /** What a claim now (or the next one) gives. */
  reward: number;
};

/**
 * Where someone stands with a DAILY rule: whether they've claimed today
 * (UTC), their streak, and what the next claim gives (the amount, plus the
 * bonus for each day in a row after the first, up to the rule's limit).
 */
export function dailyOf(balance: Pick<Balance, "dailyClaimedAt" | "dailyStreak"> | undefined, rule: Pick<EarnRule, "amount" | "streakBonus" | "streakMaxDays">, now: number): Daily {
  const last = ms(balance?.dailyClaimedAt);
  const today = utcDay(now);
  const lastDay = last ? utcDay(last) : -Infinity;
  const claimedToday = lastDay === today;
  const going = claimedToday || lastDay === today - 1;
  const streak = going ? (balance?.dailyStreak ?? 0) : 0;
  // The claim this reward is for: today's (if not yet) or tomorrow's, which continues the streak either way.
  const nextStreak = going ? streak + 1 : 1;
  const bonusDays = Math.min(nextStreak - 1, Math.max(0, rule.streakMaxDays));
  return {
    ready: !claimedToday,
    nextAt: (today + 1) * DAY_MS,
    streak,
    reward: num(rule.amount) + num(rule.streakBonus) * bonusDays,
  };
}

/** A currency's DAILY rule, if it has one. */
export const dailyRule = (currency: Pick<Currency, "earn">) => currency.earn.find((r) => r.action === DAILY);

// ── The earn rules editor ────────────────────────────────────────────────

/** One action's rule as the editor holds it: switched on or not, numbers as typed. */
export type RuleForm = { on: boolean; amount: string; cooldown: string; dailyCap: string; streakBonus: string; streakMaxDays: string; games: string };
export type RulesForm = Record<number, RuleForm>;

const OFF: RuleForm = { on: false, amount: "", cooldown: "", dailyCap: "", streakBonus: "", streakMaxDays: "", games: "" };

/** What a rule just switched on starts with. */
export const STARTER: Record<number, Omit<RuleForm, "on">> = {
  [MESSAGE]: { amount: "1", cooldown: "60", dailyCap: "", streakBonus: "", streakMaxDays: "", games: "" },
  [REACTION_RECEIVED]: { amount: "1", cooldown: "30", dailyCap: "", streakBonus: "", streakMaxDays: "", games: "" },
  [VOICE_MINUTE]: { amount: "1", cooldown: "", dailyCap: "", streakBonus: "", streakMaxDays: "", games: "" },
  [GAME_MINUTE]: { amount: "1", cooldown: "", dailyCap: "60", streakBonus: "", streakMaxDays: "", games: "" },
  [DAILY]: { amount: "50", cooldown: "", dailyCap: "", streakBonus: "10", streakMaxDays: "7", games: "" },
  [JOIN]: { amount: "100", cooldown: "", dailyCap: "", streakBonus: "", streakMaxDays: "", games: "" },
  [ONBOARDING]: { amount: "100", cooldown: "", dailyCap: "", streakBonus: "", streakMaxDays: "", games: "" },
};

const text = (n: number | bigint) => (num(n) ? String(num(n)) : "");

/** A currency's rules as the editor shows them: every action, switched on where it has a rule. */
export function rulesForm(earn: readonly Pick<EarnRule, "action" | "amount" | "cooldownSeconds" | "dailyCap" | "streakBonus" | "streakMaxDays" | "games">[]): RulesForm {
  const form: RulesForm = {};
  for (const { action } of ACTIONS) {
    const rule = earn.find((r) => r.action === action);
    form[action] = rule
      ? {
          on: true,
          amount: String(num(rule.amount)),
          cooldown: text(rule.cooldownSeconds),
          dailyCap: text(rule.dailyCap),
          streakBonus: text(rule.streakBonus),
          streakMaxDays: text(rule.streakMaxDays),
          games: rule.games.join("\n"),
        }
      : { ...OFF };
  }
  return form;
}

/** The plain shape of an EarnRule, for the request. */
export type RuleValue = { action: number; amount: bigint; cooldownSeconds: number; dailyCap: bigint; streakBonus: bigint; streakMaxDays: number; games: string[] };

/** Which field of which action is wrong. */
export type RuleProblem = { action: number; field: keyof Omit<RuleForm, "on"> };

const orZero = (s: string) => (s.trim() === "" ? "0" : s);

/**
 * A game rule's games from what was typed, one per line: trimmed, each once
 * (ignoring case, as the server matches them), or null when there are too
 * many or one is too long.
 */
export function parseGames(text: string): string[] | null {
  const seen = new Set<string>();
  const games: string[] = [];
  for (const line of text.split("\n")) {
    const game = line.trim();
    if (!game) continue;
    if ([...game].length > GAME_NAME_MAX) return null;
    const key = game.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    games.push(game);
  }
  return games.length > GAMES_MAX ? null : games;
}

/** The rules to send, from what's switched on, or the first field that isn't a number the server takes. */
export function formRules(form: RulesForm): { rules: RuleValue[] } | { problem: RuleProblem } {
  const rules: RuleValue[] = [];
  for (const meta of ACTIONS) {
    const f = form[meta.action];
    if (!f?.on) continue;
    const amount = parseAmount(f.amount, 1, AMOUNT_MAX);
    if (amount === null) return { problem: { action: meta.action, field: "amount" } };
    const cooldown = meta.cooldown ? parseAmount(orZero(f.cooldown), 0, COOLDOWN_MAX) : 0;
    if (cooldown === null) return { problem: { action: meta.action, field: "cooldown" } };
    const cap = parseAmount(orZero(f.dailyCap), 0, Number.MAX_SAFE_INTEGER);
    if (cap === null) return { problem: { action: meta.action, field: "dailyCap" } };
    const bonus = meta.streak ? parseAmount(orZero(f.streakBonus), 0, AMOUNT_MAX) : 0;
    if (bonus === null) return { problem: { action: meta.action, field: "streakBonus" } };
    const days = meta.streak ? parseAmount(orZero(f.streakMaxDays), 0, 3650) : 0;
    if (days === null) return { problem: { action: meta.action, field: "streakMaxDays" } };
    const games = meta.games ? parseGames(f.games) : [];
    if (games === null) return { problem: { action: meta.action, field: "games" } };
    rules.push({ action: meta.action, amount: BigInt(amount), cooldownSeconds: cooldown, dailyCap: BigInt(cap), streakBonus: BigInt(bonus), streakMaxDays: days, games });
  }
  return { rules };
}

/** Whether two sets of rules say the same. */
export function sameRules(a: readonly RuleValue[], b: readonly Pick<EarnRule, "action" | "amount" | "cooldownSeconds" | "dailyCap" | "streakBonus" | "streakMaxDays" | "games">[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((r) => {
    const o = b.find((x) => x.action === r.action);
    return (
      !!o &&
      num(o.amount) === num(r.amount) &&
      o.cooldownSeconds === r.cooldownSeconds &&
      num(o.dailyCap) === num(r.dailyCap) &&
      num(o.streakBonus) === num(r.streakBonus) &&
      o.streakMaxDays === r.streakMaxDays &&
      o.games.length === r.games.length &&
      o.games.every((g, i) => g === r.games[i])
    );
  });
}

// ── The currency and listing editors ─────────────────────────────────────

export type CurrencyDraft = { name: string; iconEmoji: string; iconEmojiId: string; description: string; transferable: boolean; rules: RulesForm };

export const currencyDraft = (c: Pick<Currency, "name" | "iconEmoji" | "iconEmojiId" | "description" | "transferable" | "earn"> | undefined): CurrencyDraft => ({
  name: c?.name ?? "",
  iconEmoji: c?.iconEmoji ?? (c ? "" : FALLBACK_ICON),
  iconEmojiId: c?.iconEmojiId ?? "",
  description: c?.description ?? "",
  transferable: c?.transferable ?? false,
  rules: rulesForm(c?.earn ?? []),
});

/** The fields of a CurrencyChange (unset ones stay as they are). */
export type CurrencyChangeValue = {
  name?: string;
  position?: number;
  iconEmoji?: string;
  iconEmojiId?: string;
  description?: string;
  transferable?: boolean;
  replaceEarn: boolean;
  earn: RuleValue[];
};

/**
 * What to send for a draft: everything for a new currency, only what
 * changed for one that's there, and how many things changed (for the save
 * bar). A rule that isn't a number makes it a problem instead.
 */
export function currencyChange(
  saved: Pick<Currency, "name" | "iconEmoji" | "iconEmojiId" | "description" | "transferable" | "earn"> | undefined,
  draft: CurrencyDraft,
): { change: CurrencyChangeValue; count: number } | { problem: RuleProblem | "name" } {
  const name = draft.name.trim();
  if (!name || [...name].length > NAME_MAX) return { problem: "name" };
  const formed = formRules(draft.rules);
  if ("problem" in formed) return formed;
  const description = draft.description.trim();
  if (!saved) {
    return {
      change: { name, iconEmoji: draft.iconEmoji, iconEmojiId: draft.iconEmojiId, description, transferable: draft.transferable, replaceEarn: true, earn: formed.rules },
      count: 1,
    };
  }
  const change: CurrencyChangeValue = { replaceEarn: false, earn: [] };
  let count = 0;
  if (name !== saved.name) {
    change.name = name;
    count++;
  }
  if (draft.iconEmoji !== saved.iconEmoji || draft.iconEmojiId !== saved.iconEmojiId) {
    change.iconEmoji = draft.iconEmoji;
    change.iconEmojiId = draft.iconEmojiId;
    count++;
  }
  if (description !== saved.description) {
    change.description = description;
    count++;
  }
  if (draft.transferable !== saved.transferable) {
    change.transferable = draft.transferable;
    count++;
  }
  if (!sameRules(formed.rules, saved.earn)) {
    change.replaceEarn = true;
    change.earn = formed.rules;
    count++;
  }
  return { change, count };
}

export type ListingDraft = {
  currencyId: string;
  kind: number;
  targetId: string;
  name: string;
  description: string;
  price: string;
  /** "" for no limit. */
  stock: string;
  position: string;
  available: boolean;
};

export const listingDraft = (l: ShopListing | undefined, currencyId = "", position = 0): ListingDraft => ({
  currencyId: l?.currencyId ?? currencyId,
  kind: l?.kind ?? LISTING_ROLE,
  targetId: l?.targetId ?? "",
  name: l?.name ?? "",
  description: l?.description ?? "",
  price: l ? String(num(l.price)) : "100",
  stock: l?.stock === undefined ? "" : String(num(l.stock)),
  position: String(l?.position ?? position),
  available: l?.available ?? true,
});

export type ListingProblem = "currency" | "target" | "name" | "price" | "stock" | "position";

/** A NewShopListing's fields. */
export type NewListingValue = {
  currencyId: string;
  kind: number;
  targetId: string;
  name: string;
  description: string;
  price: bigint;
  stock?: bigint;
  position: number;
  available: boolean;
};

/** A ShopListingChange's fields (unset ones stay as they are). */
export type ListingChangeValue = { name?: string; description?: string; price?: bigint; stock?: bigint; unlimited: boolean; position?: number; available?: boolean };

/** A listing to create, or what to change in one that's there and how many things changed. */
export function listingChange(
  saved: ShopListing | undefined,
  draft: ListingDraft,
): { count: number; create: NewListingValue; change: ListingChangeValue } | { problem: ListingProblem } {
  if (!draft.currencyId) return { problem: "currency" };
  if (draft.kind !== LISTING_CUSTOM && !draft.targetId) return { problem: "target" };
  const name = draft.name.trim();
  if ((draft.kind === LISTING_CUSTOM && !name) || [...name].length > LISTING_NAME_MAX) return { problem: "name" };
  const price = parseAmount(draft.price, 1, PRICE_MAX);
  if (price === null) return { problem: "price" };
  const stock = draft.stock.trim() === "" ? null : parseAmount(draft.stock, 0, Number.MAX_SAFE_INTEGER);
  if (draft.stock.trim() !== "" && stock === null) return { problem: "stock" };
  const position = parseAmount(orZero(draft.position), -1_000_000, 1_000_000);
  if (position === null) return { problem: "position" };
  const description = draft.description.trim();
  const create: NewListingValue = {
    currencyId: draft.currencyId,
    kind: draft.kind,
    targetId: draft.kind === LISTING_CUSTOM ? "" : draft.targetId,
    name,
    description,
    price: BigInt(price),
    stock: stock === null ? undefined : BigInt(stock),
    position,
    available: draft.available,
  };
  const change: ListingChangeValue = { unlimited: false };
  if (!saved) return { count: 1, create, change };
  let count = 0;
  if (name !== saved.name) {
    change.name = name;
    count++;
  }
  if (description !== saved.description) {
    change.description = description;
    count++;
  }
  if (price !== num(saved.price)) {
    change.price = BigInt(price);
    count++;
  }
  const had = stockLeft(saved);
  if (stock !== had) {
    if (stock === null) change.unlimited = true;
    else change.stock = BigInt(stock);
    count++;
  }
  if (position !== saved.position) {
    change.position = position;
    count++;
  }
  if (draft.available !== saved.available) {
    change.available = draft.available;
    count++;
  }
  return { count, create, change };
}

// ── The ledger ───────────────────────────────────────────────────────────

/** How one ledger line reads: a catalog key, and which of its people and things to name. */
export function ledgerLine(entry: Pick<LedgerEntry, "kind" | "amount" | "action" | "roleId" | "actorId">): Key {
  const out = num(entry.amount) < 0;
  switch (entry.kind) {
    case LEDGER_EARN:
      return earnedLine(entry.action);
    case LEDGER_GRANT:
      if (entry.roleId) return out ? "chattools.economy.line.takenRole" : "chattools.economy.line.givenRole";
      return entry.actorId ? (out ? "chattools.economy.line.taken" : "chattools.economy.line.given") : out ? "chattools.economy.line.takenSomeone" : "chattools.economy.line.givenSomeone";
    case LEDGER_TRANSFER:
      return out ? "chattools.economy.line.sent" : "chattools.economy.line.received";
    case LEDGER_PURCHASE:
      return "chattools.economy.line.bought";
    default:
      return "chattools.economy.line.other";
  }
}

const earnedLine = (action: number): Key => {
  switch (action) {
    case MESSAGE:
      return "chattools.economy.line.earnMessage";
    case REACTION_RECEIVED:
      return "chattools.economy.line.earnReaction";
    case VOICE_MINUTE:
      return "chattools.economy.line.earnVoice";
    case GAME_MINUTE:
      return "chattools.economy.line.earnGame";
    case DAILY:
      return "chattools.economy.line.earnDaily";
    case JOIN:
      return "chattools.economy.line.earnJoin";
    case ONBOARDING:
      return "chattools.economy.line.earnOnboarding";
    default:
      return "chattools.economy.line.earn";
  }
};

/** A ledger page added to what's loaded, newest first, each entry once. */
export function withPage<T extends { id: string }>(list: readonly T[], page: readonly T[]): T[] {
  const ids = new Set(list.map((e) => e.id));
  return [...list, ...page.filter((e) => !ids.has(e.id))].sort((a, b) => (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));
}

// ── The finance page ─────────────────────────────────────────────────────

/** A round top for a chart's scale, at or above the biggest value: 1, 2 or 5 times a power of ten. */
export function niceMax(value: number): number {
  if (value <= 0) return 1;
  const power = 10 ** Math.floor(Math.log10(value));
  for (const step of [1, 2, 5, 10]) if (step * power >= value) return step * power;
  return 10 * power;
}

/** Each day's columns, as fractions of the chart's height. */
export function chartColumns(days: readonly { earned: bigint | number; granted: bigint | number; spent: bigint | number }[]): {
  max: number;
  columns: { earned: number; granted: number; spent: number }[];
} {
  const biggest = Math.max(0, ...days.flatMap((d) => [num(d.earned), num(d.granted), num(d.spent)]));
  const max = niceMax(biggest);
  return { max, columns: days.map((d) => ({ earned: num(d.earned) / max, granted: num(d.granted) / max, spent: num(d.spent) / max })) };
}

/** A share of a total, 0 to 1, for bars in ranked lists. */
export const share = (part: number, total: number) => (total > 0 ? Math.min(1, Math.max(0, part / total)) : 0);

/* ---- The server's refusals, in the app's words ---- */

/** gRPC's codes for what the economy refuses (Connect's `Code`, kept here so this file stays pure). */
const ALREADY_EXISTS = 6;
const FAILED_PRECONDITION = 9;

/** A refusal worded by the app: a key and what fills it. */
export type Worded = { key: Key; values?: Record<string, string> };

/**
 * What the server's economy refusals mean, worded by the app instead of the
 * server's English. Matched by code and then by the message the server
 * writes; anything else (or a refusal worded differently by another
 * version) is null, and the server's own message is shown.
 */
export function economyProblem(err: { code: number; message: string }): Worded | null {
  const message = err.message.trim();
  if (err.code === ALREADY_EXISTS) {
    return { key: /\brole\b/.test(message) ? "chattools.economy.problem.haveRole" : "chattools.economy.problem.haveItem" };
  }
  if (err.code !== FAILED_PRECONDITION) return null;
  let found: RegExpMatchArray | null;
  if ((found = message.match(/^you don't have enough (.+?)\.?$/))) return { key: "chattools.economy.notEnough", values: { currency: found[1]! } };
  if (/^the price changed/.test(message)) return { key: "chattools.economy.problem.priceChanged" };
  if (/^it's sold out/.test(message)) return { key: "chattools.economy.problem.soldOut" };
  if (/claimed today's reward already/.test(message)) return { key: "chattools.economy.problem.claimed" };
  if ((found = message.match(/^(.+) can't be sent between members/))) return { key: "chattools.economy.problem.notTransferable", values: { currency: found[1]! } };
  if ((found = message.match(/^(.+) has no daily reward/))) return { key: "chattools.economy.problem.noDaily", values: { currency: found[1]! } };
  if (/can't hold any more/.test(message)) return { key: "chattools.economy.problem.full" };
  if (/sold in the server's shop/.test(message)) return { key: "chattools.economy.problem.buyFirst" };
  if (/can't carry permissions/.test(message)) return { key: "chattools.economy.problem.powerfulRole" };
  return null;
}
