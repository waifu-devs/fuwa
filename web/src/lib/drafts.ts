/**
 * What you started typing and didn't send, per account and channel (or
 * thread), for as long as the tab is open. Kept apart per account, so one
 * account's half-written message never shows in another's box.
 */

const drafts = new Map<string, string>();

/** `account` is "<instance>|<user id>"; `place` a channel id or "thread:<id>". */
export const draftKey = (account: string, place: string) => `${account}/${place}`;

export const getDraft = (key: string) => drafts.get(key) ?? "";

export function setDraft(key: string, text: string) {
  if (text) drafts.set(key, text);
  else drafts.delete(key);
}

/** Forgets every draft of one account (signing out), or of every account on an instance (`account` ending in "|"). */
export function forgetDrafts(account: string) {
  const prefix = account.endsWith("|") ? account : `${account}/`;
  for (const key of [...drafts.keys()]) if (key.startsWith(prefix)) drafts.delete(key);
}
