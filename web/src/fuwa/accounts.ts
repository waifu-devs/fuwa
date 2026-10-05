import { wipeDms } from "@/e2ee/engine";
import { accountKeys, adoptInstanceKeys as adopt, instanceKeys, RECENT_GIFS, type KeyStorage } from "@/lib/account-keys";
import { forgetDrafts } from "@/lib/drafts";
import { forgetRecentSearches } from "@/lib/search-query";
import { forgetRecentGifs } from "./gifs";
import { accountKey } from "./saved";

/**
 * What this browser keeps for each account on its own, beyond its session:
 * found by "<instance>|<user id>", so signing out of one account forgets only
 * its things and switching never shows one account another's.
 */

function storage(): KeyStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function remove(at: KeyStorage, keys: string[]) {
  for (const k of keys) {
    // Recent GIFs are also held in memory.
    if (k.startsWith(RECENT_GIFS)) forgetRecentGifs(k.slice(RECENT_GIFS.length));
    else at.removeItem(k);
  }
}

/** Hands what was kept for an instance before accounts were told apart to the account it belonged to, the only one back then. */
export function adoptInstanceKeys(key: string, userId: string) {
  const at = storage();
  try {
    if (at) adopt(at, key, accountKey(key, userId));
  } catch {
    // Storage blocked: they're shown again, nothing worse.
  }
}

/** Forgets what this browser kept for one account (signed out, its session ended, or deleted). */
export async function forgetAccount(key: string, userId: string) {
  const account = accountKey(key, userId);
  const at = storage();
  forgetDrafts(account);
  forgetRecentGifs(account);
  forgetRecentSearches(`${key}/${userId}`);
  try {
    if (at) remove(at, accountKeys(at, account));
  } catch {
    // Nothing kept, nothing to forget.
  }
  await wipeDms(key, userId);
}

/** Forgets what this browser kept for every account on an instance (forgetting the instance). */
export async function forgetInstance(key: string) {
  const at = storage();
  forgetDrafts(accountKey(key, ""));
  forgetRecentSearches(key);
  try {
    if (at) remove(at, instanceKeys(at, key));
  } catch {
    // Nothing kept, nothing to forget.
  }
  await wipeDms(key);
}
