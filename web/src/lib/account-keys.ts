/**
 * Which localStorage keys hold one account's own things (applications
 * waiting, recent GIFs, welcome screens seen), found by "<instance>|<user id>",
 * and the move of what was kept per instance, before accounts were told
 * apart, to the account it belonged to.
 */

export type KeyStorage = {
  readonly length: number;
  key(n: number): string | null;
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
};

export const APPLIED = "fuwa:applied:";
export const RECENT_GIFS = "fuwa.gifs.recent.";
/** Followed by "<account>.<server id>". */
export const WELCOMED = "fuwa.welcomed.";

const OWN = [APPLIED, RECENT_GIFS] as const;

function allKeys(at: KeyStorage): string[] {
  const out: string[] = [];
  for (let n = 0; n < at.length; n++) {
    const key = at.key(n);
    if (key !== null) out.push(key);
  }
  return out;
}

/** Keys starting with `prefix` and then a server id (no dots), so another instance whose address starts the same never matches. */
const serverKeys = (keys: string[], prefix: string) => keys.filter((k) => k.startsWith(prefix) && !k.slice(prefix.length).includes("."));

/** Hands what was kept for instance `key` to `account` ("<instance>|<user id>"); what the account already has wins. */
export function adoptInstanceKeys(at: KeyStorage, key: string, account: string) {
  for (const prefix of OWN) {
    const old = at.getItem(prefix + key);
    if (old === null) continue;
    if (at.getItem(prefix + account) === null) at.setItem(prefix + account, old);
    at.removeItem(prefix + key);
  }
  for (const old of serverKeys(allKeys(at), `${WELCOMED}${key}.`)) {
    at.setItem(`${WELCOMED}${account}.${old.slice(`${WELCOMED}${key}.`.length)}`, "1");
    at.removeItem(old);
  }
}

/** The keys one account's things are under. */
export const accountKeys = (at: KeyStorage, account: string): string[] => [
  ...OWN.map((prefix) => prefix + account).filter((k) => at.getItem(k) !== null),
  ...serverKeys(allKeys(at), `${WELCOMED}${account}.`),
];

/** The keys every account's things on an instance are under, and what was kept for it before accounts were told apart. */
export function instanceKeys(at: KeyStorage, key: string): string[] {
  const keys = allKeys(at);
  return [
    ...keys.filter((k) => [...OWN, WELCOMED].some((prefix) => k === prefix + key || k.startsWith(`${prefix}${key}|`))),
    ...serverKeys(keys, `${WELCOMED}${key}.`),
  ];
}
