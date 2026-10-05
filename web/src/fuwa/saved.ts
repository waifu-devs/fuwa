/**
 * The instances this browser knows about and the accounts signed in to each.
 * An instance can keep several accounts; one is active (connected) at a
 * time. They live in localStorage and never leave the browser, except that
 * each token goes to its own instance. Every change reads the list again
 * first, so tabs adding accounts at once don't drop each other's.
 */

const KEY = "fuwa:accounts:v2";
/** Before several accounts: one token per instance. */
const OLD_KEY = "fuwa:instances:v1";

/** One account on an instance, with a small card to show in the switcher while it isn't connected. */
export type SavedAccount = {
  /** Empty for a session kept from before accounts were told apart, until the instance says whose it is. */
  userId: string;
  token: string;
  username: string;
  displayName: string;
  avatarUrl: string;
};

export type SavedInstance = {
  url: string;
  /** The account that's connected, by user id; null when signed out. */
  active: string | null;
  accounts: SavedAccount[];
};

type Storage = { getItem(key: string): string | null; setItem(key: string, value: string): void; removeItem(key: string): void };

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

const isString = (v: unknown): v is string => typeof v === "string";

function isAccount(a: unknown): a is SavedAccount {
  const x = a as Partial<SavedAccount> | null;
  return !!x && isString(x.userId) && isString(x.token) && !!x.token && isString(x.username) && isString(x.displayName) && isString(x.avatarUrl);
}

function parse(raw: string | null): SavedInstance[] | null {
  if (!raw) return null;
  try {
    const list = JSON.parse(raw) as unknown;
    if (!Array.isArray(list)) return [];
    return list.flatMap((i: Partial<SavedInstance> | null) => {
      if (!i || !isString(i.url)) return [];
      const accounts = Array.isArray(i.accounts) ? i.accounts.filter(isAccount) : [];
      const active = isString(i.active) && accounts.some((a) => a.userId === i.active) ? i.active : null;
      return [{ url: i.url, active, accounts }];
    });
  } catch {
    return [];
  }
}

/** A list from before several accounts, as instances with one account whose user isn't known yet. */
function fromOld(raw: string | null): SavedInstance[] {
  try {
    const list = raw ? (JSON.parse(raw) as unknown) : [];
    if (!Array.isArray(list)) return [];
    return list.flatMap((i: { url?: unknown; token?: unknown } | null): SavedInstance[] => {
      if (!i || !isString(i.url)) return [];
      if (!isString(i.token) || !i.token) return [{ url: i.url, active: null, accounts: [] }];
      return [{ url: i.url, active: "", accounts: [{ userId: "", token: i.token, username: "", displayName: "", avatarUrl: "" }] }];
    });
  } catch {
    return [];
  }
}

export function loadSaved(from: Storage | null = storage()): SavedInstance[] {
  if (!from) return [];
  try {
    return parse(from.getItem(KEY)) ?? fromOld(from.getItem(OLD_KEY));
  } catch {
    return [];
  }
}

export function storeSaved(list: SavedInstance[], to: Storage | null = storage()) {
  if (!to) return;
  try {
    to.setItem(KEY, JSON.stringify(list));
    to.removeItem(OLD_KEY);
  } catch {
    // Private mode or storage full: the session still works until the tab closes.
  }
}

/** Changes the kept list as it is now (another tab may have changed it), and returns the new one. */
export function updateSaved(fn: (list: SavedInstance[]) => SavedInstance[], at: Storage | null = storage()): SavedInstance[] {
  const before = loadSaved(at);
  const next = fn(before);
  if (next !== before) storeSaved(next, at);
  return next;
}

/** Whether a storage event is about the kept accounts. */
export const isSavedKey = (key: string | null) => key === KEY;

const sameInstance = (a: string, b: string) => instanceKey(a) === instanceKey(b);

export const savedInstance = (list: SavedInstance[], url: string) => list.find((i) => sameInstance(i.url, url));

/** The account connected on an instance, if any. */
export const activeAccount = (i: SavedInstance | undefined): SavedAccount | undefined =>
  i?.active === null || !i ? undefined : i.accounts.find((a) => a.userId === i.active);

function change(list: SavedInstance[], url: string, fn: (i: SavedInstance) => SavedInstance): SavedInstance[] {
  const at = list.findIndex((i) => sameInstance(i.url, url));
  if (at === -1) return [...list, fn({ url, active: null, accounts: [] })];
  return list.map((i, n) => (n === at ? fn(i) : i));
}

export type Card = Pick<SavedAccount, "userId" | "username" | "displayName" | "avatarUrl">;

/**
 * A picture link kept for the switcher only when it's on the instance itself
 * (a path, or its own origin), so showing a kept account never makes the
 * browser fetch from anywhere else.
 */
export function ownPicture(link: string, url: string): string {
  if (!link) return "";
  if (link.startsWith("/") && !link.startsWith("//")) return link;
  try {
    return new URL(link).origin === new URL(url).origin ? link : "";
  } catch {
    return "";
  }
}

/** The sessions a change would drop from an instance: those it replaces for the same person. They should be ended on the server. */
export function replacedTokens(before: SavedInstance[], after: SavedInstance[], url: string): string[] {
  const kept = new Set((savedInstance(after, url)?.accounts ?? []).map((a) => a.token));
  return (savedInstance(before, url)?.accounts ?? []).map((a) => a.token).filter((t) => !kept.has(t));
}

/**
 * Signed in: keeps the account (replacing its old session when it was
 * already here, or a session from before accounts were told apart that
 * turns out to be the same person) and makes it the active one.
 */
export function withAccount(list: SavedInstance[], url: string, token: string, card: Card): SavedInstance[] {
  return change(list, url, (i) => {
    const account: SavedAccount = { ...card, avatarUrl: ownPicture(card.avatarUrl, url), token };
    const others = i.accounts.filter((a) => a.userId !== card.userId && a.token !== token);
    return { ...i, active: card.userId, accounts: [...others, account] };
  });
}

/**
 * The instance said whose a session is: fills in or refreshes its card.
 * Returns the same list when nothing changed, and whether the session was
 * one kept from before accounts were told apart.
 */
export function withCard(list: SavedInstance[], url: string, token: string, card: Card): { list: SavedInstance[]; wasUnknown: boolean } {
  const inst = savedInstance(list, url);
  const account = inst?.accounts.find((a) => a.token === token);
  if (!inst || !account) return { list, wasUnknown: false };
  card = { ...card, avatarUrl: ownPicture(card.avatarUrl, url) };
  const same =
    account.userId === card.userId &&
    account.username === card.username &&
    account.displayName === card.displayName &&
    account.avatarUrl === card.avatarUrl;
  if (same) return { list, wasUnknown: false };
  const wasUnknown = account.userId === "";
  // Signed in to the same account twice from before: keep the session in use.
  const accounts = inst.accounts
    .filter((a) => a === account || a.userId !== card.userId)
    .map((a) => (a === account ? { ...a, ...card } : a));
  const active = inst.active === account.userId ? card.userId : inst.active;
  return { list: change(list, url, (i) => ({ ...i, active, accounts })), wasUnknown };
}

/** Switches the instance to another kept account. */
export function withActive(list: SavedInstance[], url: string, userId: string): SavedInstance[] {
  const inst = savedInstance(list, url);
  if (!inst || inst.active === userId || !inst.accounts.some((a) => a.userId === userId)) return list;
  return change(list, url, (i) => ({ ...i, active: userId }));
}

/**
 * Forgets one account (signed out, its session ended, or deleted). The
 * instance stays, signed out, and nothing else becomes active on its own:
 * the person picks who to continue as.
 */
export function withoutAccount(list: SavedInstance[], url: string, userId: string): SavedInstance[] {
  const inst = savedInstance(list, url);
  if (!inst?.accounts.some((a) => a.userId === userId)) return list;
  return change(list, url, (i) => ({
    ...i,
    active: i.active === userId ? null : i.active,
    accounts: i.accounts.filter((a) => a.userId !== userId),
  }));
}

/** Keeps an instance listed with nobody signed in, if it isn't already. */
export const withInstance = (list: SavedInstance[], url: string): SavedInstance[] =>
  savedInstance(list, url) ? list : [...list, { url, active: null, accounts: [] }];

export const withoutInstance = (list: SavedInstance[], url: string): SavedInstance[] =>
  list.filter((i) => !sameInstance(i.url, url));

/** Turns whatever someone typed ("fuwa.waifu.dev", "localhost:8080/") into a base URL. */
export function normalizeUrl(input: string): string {
  let text = input.trim();
  if (!/^https?:\/\//i.test(text)) {
    const local = /^(localhost|127\.|\[::1\]|0\.0\.0\.0|10\.|192\.168\.)/i.test(text);
    text = `${local ? "http" : "https"}://${text}`;
  }
  const url = new URL(text);
  return url.origin + url.pathname.replace(/\/+$/, "");
}

/** A short, readable id for an instance, used in the address bar: "fuwa.waifu.dev". */
export function instanceKey(url: string): string {
  const u = new URL(url);
  return (u.host + u.pathname.replace(/\/+$/, "")).replaceAll("/", "~");
}

/** Where one account's own things are kept on this device: "<instance>|<user id>", like its encrypted messages. */
export const accountKey = (key: string, userId: string) => `${key}|${userId}`;
