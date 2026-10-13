import { useEffect, useSyncExternalStore } from "react";
import type { EconomyOverview, ListLedgerResponse } from "@/gen/fuwa/v1/economy_pb";
import type { Balance, Currency, Event, LedgerKind, ShopListing, ShopListingKind } from "@/gen/fuwa/v1/types_pb";
import { instanceHas } from "@/lib/compat";
import {
  applyEconomy,
  ECONOMY_EVENTS,
  EMPTY,
  LEDGER_PAGE,
  newWallet,
  sortCurrencies,
  sortListings,
  withBalance,
  withListing,
  withOwned,
  LISTING_PROFILE_ITEM,
  type CurrencyChangeValue,
  type EconomyPayload,
  type ListingChangeValue,
  type NewListingValue,
  type ServerEconomy,
} from "@/lib/economy";
import { reportUsage } from "@/lib/reports";
import { toFuwaError } from "./errors";
import { store, useFuwa } from "./store";
import { engine } from "./sync";

/*
 * A server's economy (docs/economy.md): its currencies, its shop and your
 * wallet, kept apart from the main store like Mod View's counts. Currencies
 * and the wallet load with each server on instances that have the economy;
 * the shop loads when something shows it. Events keep all three in step
 * (lib/economy.ts's `applyEconomy`), and the calls below fold their answers
 * in at once, so the screen moves before the event comes back.
 */

const call = async <A>(work: () => Promise<A>): Promise<A> => {
  try {
    return await work();
  } catch (err) {
    throw toFuwaError(err);
  }
};

const api = (instanceKey: string) => engine(instanceKey).api.economy;
const meOf = (instanceKey: string) => store.get().instances[instanceKey]?.me?.id ?? "";

/** Whether an instance has the economy, so its screens may show. */
export const economyHere = (instanceKey: string) => instanceHas(store.get().instances[instanceKey]?.node?.versions, "economy");

// ── The state, per server ────────────────────────────────────────────────

const states = new Map<string, ServerEconomy>();
const listeners = new Set<() => void>();
const subscribe = (l: () => void) => (listeners.add(l), () => listeners.delete(l));
const keyOf = (instanceKey: string, serverId: string) => `${instanceKey}|${serverId}`;

/** A server's economy as known now. */
export const economyNow = (instanceKey: string, serverId: string): ServerEconomy => states.get(keyOf(instanceKey, serverId)) ?? EMPTY;

function update(instanceKey: string, serverId: string, fn: (s: ServerEconomy) => ServerEconomy) {
  const key = keyOf(instanceKey, serverId);
  const had = states.get(key) ?? EMPTY;
  const next = fn(had);
  if (next === had) return;
  states.set(key, next);
  for (const l of listeners) l();
}

const loading = new Set<string>();

/** Reads a server's currencies and, when it has any, your wallet. Several asks at once read once. */
export function loadEconomy(instanceKey: string, serverId: string): Promise<void> {
  const key = keyOf(instanceKey, serverId);
  if (loading.has(key)) return Promise.resolve();
  loading.add(key);
  return (async () => {
    try {
      const { currencies } = await api(instanceKey).listCurrencies({ serverId });
      update(instanceKey, serverId, (s) => ({ ...s, currencies: sortCurrencies(currencies) }));
      if (currencies.length) await readWallet(instanceKey, serverId);
      // Loaded again (joined anew): the shop shown follows.
      if (economyNow(instanceKey, serverId).shop) await loadShop(instanceKey, serverId);
    } catch {
      // Shown again the next time something asks.
    } finally {
      loading.delete(key);
    }
  })();
}

async function readWallet(instanceKey: string, serverId: string) {
  const res = await api(instanceKey).getWallet({ serverId, userId: "" });
  update(instanceKey, serverId, (s) => ({ ...s, wallet: newWallet(res.balances, res.ownedItemIds) }));
}

const shopLoading = new Set<string>();
/** Reads the shop (everything, for managers; what's for sale, for members). */
export function loadShop(instanceKey: string, serverId: string): Promise<void> {
  const key = keyOf(instanceKey, serverId);
  if (shopLoading.has(key)) return Promise.resolve();
  shopLoading.add(key);
  return api(instanceKey)
    .listShop({ serverId })
    .then(
      ({ listings }) => update(instanceKey, serverId, (s) => ({ ...s, shop: sortListings(listings) })),
      () => {},
    )
    .finally(() => shopLoading.delete(key));
}

const timers = new Map<string, ReturnType<typeof setTimeout>>();
/** Several wallet changes at once (grants to a role) read the wallet once. */
function rereadSoon(instanceKey: string, serverId: string) {
  const key = keyOf(instanceKey, serverId);
  if (timers.has(key)) return;
  timers.set(
    key,
    setTimeout(() => {
      timers.delete(key);
      readWallet(instanceKey, serverId).catch(() => {});
    }, 500),
  );
}

/**
 * A server's economy, read when first shown on an instance that has it.
 * `shop` reads the shop too.
 */
export function useEconomy(instanceKey: string, serverId: string, shop = false): ServerEconomy {
  const state = useSyncExternalStore(subscribe, () => economyNow(instanceKey, serverId));
  const here = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "economy"));
  const missing = here && !!serverId && state.currencies === null;
  const shopMissing = here && !!serverId && shop && state.shop === null;
  useEffect(() => {
    if (missing) void loadEconomy(instanceKey, serverId);
  }, [missing, instanceKey, serverId]);
  useEffect(() => {
    if (shopMissing) void loadShop(instanceKey, serverId);
  }, [shopMissing, instanceKey, serverId]);
  return state;
}

/** A currency by id from any server known (ids are unique), for lines like the audit log's. */
export function anyCurrency(currencyId: string): Currency | undefined {
  for (const s of states.values()) {
    const found = s.currencies?.find((c) => c.id === currencyId);
    if (found) return found;
  }
  return undefined;
}

/** A currency by id, from what's known. */
export const currencyOf = (instanceKey: string, serverId: string, currencyId: string): Currency | undefined =>
  economyNow(instanceKey, serverId).currencies?.find((c) => c.id === currencyId);

// ── Members' calls ───────────────────────────────────────────────────────

function keepBalance(instanceKey: string, serverId: string, balance: Balance | undefined, entryId = "") {
  if (!balance) return;
  update(instanceKey, serverId, (s) => (s.wallet ? { ...s, wallet: withBalance(s.wallet, balance, entryId) } : s));
}

export async function claimDaily(instanceKey: string, serverId: string, currencyId: string) {
  reportUsage("economy.claim");
  const res = await call(() => api(instanceKey).claimDaily({ serverId, currencyId }));
  keepBalance(instanceKey, serverId, res.balance, res.entry?.id);
  return res;
}

export async function transfer(instanceKey: string, serverId: string, currencyId: string, userId: string, amount: number, note: string) {
  reportUsage("economy.transfer");
  const res = await call(() => api(instanceKey).transfer({ serverId, currencyId, userId, amount: BigInt(amount), note }));
  keepBalance(instanceKey, serverId, res.balance, res.entry?.id);
  return res;
}

export async function buy(instanceKey: string, serverId: string, listing: ShopListing) {
  reportUsage("economy.buy");
  const res = await call(() => api(instanceKey).buy({ serverId, listingId: listing.id, expectedPrice: listing.price }));
  keepBalance(instanceKey, serverId, res.balance, res.entry?.id);
  const bought = res.listing;
  update(instanceKey, serverId, (s) => {
    let next = s;
    if (bought && s.shop) next = { ...next, shop: withListing(s.shop, bought) };
    if (listing.kind === LISTING_PROFILE_ITEM && next.wallet) next = { ...next, wallet: withOwned(next.wallet, listing.targetId) };
    return next;
  });
  return res;
}

/** Changes to balances, newest first: yours, someone's or (`userId` "*") everyone's. */
export const listLedger = (
  instanceKey: string,
  serverId: string,
  filter: { currencyId?: string; userId?: string; kinds?: number[]; beforeId?: string },
): Promise<ListLedgerResponse> =>
  call(() =>
    api(instanceKey).listLedger({
      serverId,
      currencyId: filter.currencyId ?? "",
      userId: filter.userId ?? "",
      kinds: (filter.kinds ?? []) as LedgerKind[],
      beforeId: filter.beforeId ?? "",
      limit: LEDGER_PAGE,
    }),
  );

/** Someone's wallet (Manage Economy or Grant Currency). */
export const walletOf = (instanceKey: string, serverId: string, userId: string) => call(() => api(instanceKey).getWallet({ serverId, userId }));

// ── Managers' calls ──────────────────────────────────────────────────────

const keepCurrency = (instanceKey: string, serverId: string, currency: Currency | undefined) =>
  currency &&
  update(instanceKey, serverId, (s) => {
    const list = s.currencies ?? [];
    const at = list.findIndex((c) => c.id === currency.id);
    return { ...s, currencies: sortCurrencies(at === -1 ? [...list, currency] : list.map((c, n) => (n === at ? currency : c))) };
  });

export async function createCurrency(instanceKey: string, serverId: string, change: CurrencyChangeValue) {
  reportUsage("economy.currency_create");
  const res = await call(() => api(instanceKey).createCurrency({ serverId, currency: change }));
  keepCurrency(instanceKey, serverId, res.currency);
  // A first currency: the wallet has a balance in it now.
  if (!economyNow(instanceKey, serverId).wallet) await readWallet(instanceKey, serverId).catch(() => {});
  return res.currency;
}

export async function updateCurrency(instanceKey: string, serverId: string, currencyId: string, change: CurrencyChangeValue) {
  const res = await call(() => api(instanceKey).updateCurrency({ serverId, currencyId, change }));
  keepCurrency(instanceKey, serverId, res.currency);
  return res.currency;
}

export async function deleteCurrency(instanceKey: string, serverId: string, currencyId: string) {
  await call(() => api(instanceKey).deleteCurrency({ serverId, currencyId }));
  update(instanceKey, serverId, (s) => ({
    currencies: s.currencies?.filter((c) => c.id !== currencyId) ?? null,
    shop: s.shop?.filter((l) => l.currencyId !== currencyId) ?? null,
    wallet: s.wallet ? { ...s.wallet, balances: s.wallet.balances.filter((b) => b.currencyId !== currencyId) } : null,
  }));
}

const keepListing = (instanceKey: string, serverId: string, listing: ShopListing | undefined) =>
  listing && update(instanceKey, serverId, (s) => ({ ...s, shop: withListing(s.shop ?? [], listing) }));

export async function createListing(instanceKey: string, serverId: string, listing: NewListingValue) {
  reportUsage("economy.listing_create");
  const res = await call(() => api(instanceKey).createShopListing({ serverId, listing: { ...listing, kind: listing.kind as ShopListingKind } }));
  keepListing(instanceKey, serverId, res.listing);
  return res.listing;
}

export async function updateListing(instanceKey: string, serverId: string, listingId: string, change: ListingChangeValue) {
  const res = await call(() => api(instanceKey).updateShopListing({ serverId, listingId, change }));
  keepListing(instanceKey, serverId, res.listing);
  return res.listing;
}

export async function deleteListing(instanceKey: string, serverId: string, listingId: string) {
  await call(() => api(instanceKey).deleteShopListing({ serverId, listingId }));
  update(instanceKey, serverId, (s) => ({ ...s, shop: s.shop?.filter((l) => l.id !== listingId) ?? null }));
}

/** The finance page for one currency over the last `days`. */
export const economyOverview = async (instanceKey: string, serverId: string, currencyId: string, days: number): Promise<EconomyOverview | undefined> =>
  (await call(() => api(instanceKey).getEconomyOverview({ serverId, currencyId, days }))).overview;

/** Gives (or, below zero, takes) an amount to a member or everyone with a role. */
export async function grant(instanceKey: string, serverId: string, to: { userId?: string; roleId?: string }, currencyId: string, amount: number, reason: string) {
  reportUsage(to.roleId ? "economy.grant_role" : "economy.grant");
  return call(() => api(instanceKey).grant({ serverId, currencyId, userId: to.userId ?? "", roleId: to.roleId ?? "", amount: BigInt(amount), reason }));
}

// ── Events ───────────────────────────────────────────────────────────────

/** A server's event: the economy's change what's kept here. */
export function onEconomyEvent(instanceKey: string, event: Event) {
  const p = event.payload;
  if (!p.case || !ECONOMY_EVENTS.has(p.case)) return;
  const serverId = event.serverId;
  let reread = false;
  update(instanceKey, serverId, (s) => {
    const out = applyEconomy(s, p as EconomyPayload, meOf(instanceKey));
    reread = out.reread;
    return out.state;
  });
  if (reread) rereadSoon(instanceKey, serverId);
}

/** Forgets a server's economy (left, or deleted). */
export function forgetEconomyServer(instanceKey: string, serverId: string) {
  if (states.delete(keyOf(instanceKey, serverId))) for (const l of listeners) l();
}

/** Forgets an instance's economies, when it's removed or signed out of. */
export function forgetEconomy(instanceKey: string) {
  for (const key of [...states.keys()]) if (key.startsWith(`${instanceKey}|`)) states.delete(key);
  for (const l of listeners) l();
}

// ── The shop, opened from anywhere ───────────────────────────────────────

export type ShopTab = "shop" | "history" | "send";
type ShopOpen = { instanceKey: string; serverId: string; tab: ShopTab; n: number };

let shopView: { open: boolean; last: ShopOpen | null } = { open: false, last: null };
const viewListeners = new Set<() => void>();
const setShopView = (next: typeof shopView) => {
  shopView = next;
  for (const l of viewListeners) l();
};
let opened = 0;

/** Opens a server's shop, on a tab. */
export const openShop = (instanceKey: string, serverId: string, tab: ShopTab = "shop") =>
  setShopView({ open: true, last: { instanceKey, serverId, tab, n: ++opened } });
export const closeShop = () => setShopView({ ...shopView, open: false });

/** Whether the shop is open, and where it opened last (kept while it closes). */
export const useShopView = () =>
  useSyncExternalStore(
    (l) => (viewListeners.add(l), () => viewListeners.delete(l)),
    () => shopView,
  );
