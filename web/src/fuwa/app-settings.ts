import { Code } from "@connectrpc/connect";
import { instanceHas } from "@/lib/compat";
import { getPrefs, getStamps, subscribeSyncedChanges, takeSyncedPrefs } from "@/lib/prefs";
import { localEntries, merge, readDocument, writeDocument, type Entries } from "@/lib/settings-sync";
import { toFuwaError } from "./errors";
import { store } from "./store";
import { engine } from "./sync";

/**
 * App settings on every device (docs/app-settings.md): each instance you're
 * signed in to keeps a copy on your account, and this device merges its own
 * with each one's, the newest change to each setting winning.
 */

/** What each instance and account had when last read or written. */
const remotes = new Map<string, { version: bigint; entries: Entries }>();
/** One exchange at a time per instance, in order. */
const queues = new Map<string, Promise<void>>();

/** How many times a write is merged again after another device's got there first. */
const ATTEMPTS = 3;
/** How long after the last change here it goes out. */
const SEND_AFTER_MS = 2000;

const here = (key: string) => {
  const i = store.get().instances[key];
  return i?.me && getPrefs().syncSettings && instanceHas(i.node?.versions, "app-settings-sync") ? `${key}/${i.me.id}` : null;
};

const liveKeys = () => Object.entries(store.get().instances).flatMap(([key, i]) => (i.connection === "live" ? [key] : []));

/** Reads every instance's copy again, as when sync is turned back on. */
export function pullAllAppSettings() {
  for (const key of liveKeys()) void pullAppSettings(key);
}

/** Reads the instance's copy, takes what's newer there and sends what's newer here. */
export const pullAppSettings = (key: string) => exchange(key, true);

function exchange(key: string, read: boolean): Promise<void> {
  const next = (queues.get(key) ?? Promise.resolve()).then(() => settle(key, read)).catch(() => {});
  queues.set(key, next);
  return next;
}

async function settle(key: string, read: boolean) {
  const at = here(key);
  if (!at) return;
  const account = engine(key).api.account;
  for (let attempt = 0; attempt < ATTEMPTS; attempt++) {
    let remote = remotes.get(at);
    if (read || !remote) {
      const res = await account.getAppSettings({});
      remote = { version: res.version, entries: readDocument(res.document) };
      remotes.set(at, remote);
    }
    // Signed out or switched account meanwhile: this answer is someone else's.
    if (here(key) !== at) return;
    const { merged, incoming, behind } = merge(localEntries(getPrefs(), getStamps()), remote.entries);
    takeSyncedPrefs(incoming);
    // What this instance had newer goes on to the others.
    if (Object.keys(incoming).length) for (const other of liveKeys()) if (other !== key) void exchange(other, false);
    if (!behind) return;
    try {
      const res = await account.setAppSettings({ document: writeDocument(merged), version: remote.version });
      remotes.set(at, { version: res.version, entries: merged });
      return;
    } catch (err) {
      if (toFuwaError(err).code !== Code.FailedPrecondition) throw err;
      read = true;
    }
  }
}

/** Sends changes made here to every instance shortly after the last one. */
export function sendAppSettingsOnChange() {
  let timer: ReturnType<typeof setTimeout> | undefined;
  subscribeSyncedChanges(() => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      for (const key of liveKeys()) void exchange(key, false);
    }, SEND_AFTER_MS);
  });
}
