import { store } from "@/fuwa/store";
import { getPrefs, type Prefs } from "@/lib/prefs";

/**
 * Streamer mode hides what a stream shouldn't show. For fuwa that's mostly
 * instance addresses, which for a self-hoster can be a home IP or a private
 * domain, and your own username.
 */

export const hidesPersonal = (p: Prefs = getPrefs()) => p.streamer && p.streamerHidePersonal;

/** "shixzie" as "s••••••": the first letter and a dot for each other one. */
export function maskName(name: string) {
  const letters = Array.from(name);
  return (letters[0] ?? "") + "•".repeat(Math.min(Math.max(letters.length - 1, 3), 10));
}

export const HIDDEN_ADDRESS = "address hidden";

/** An instance's address as it may be shown right now. */
export const shownAddress = (key: string, p: Prefs = getPrefs()) => (hidesPersonal(p) ? HIDDEN_ADDRESS : key);

// ───────────────────────── Addresses in the address bar ─────────────────────────
// Routes name an instance by its host. In streamer mode the address bar shows
// a local alias made from its name instead, like /~waifu-devs/…, and links
// point there too. Aliases resolve back to the host in any mode, so a link
// copied during a stream still works afterwards.

const ALIAS = "~";
const ALIASES_KEY = "fuwa:aliases";

/**
 * Aliases for instances with a name, kept on this device so a reload (before
 * instances have said their names again) and older links keep working.
 */
const saved = new Map<string, string>(loadSaved());
/** Every alias handed out this visit, saved ones included. */
const seen = new Map<string, string>(saved);

function loadSaved(): [string, string][] {
  try {
    const stored = JSON.parse(localStorage.getItem(ALIASES_KEY) ?? "{}") as Record<string, unknown>;
    return Object.entries(stored).filter((e): e is [string, string] => typeof e[1] === "string");
  } catch {
    return [];
  }
}

function save(alias: string, key: string) {
  if (saved.get(alias) === key) return;
  saved.set(alias, key);
  try {
    localStorage.setItem(ALIASES_KEY, JSON.stringify(Object.fromEntries(saved)));
  } catch {
    // Kept for this visit only.
  }
}

function slug(name: string) {
  return (
    name
      .toLowerCase()
      .normalize("NFKD")
      .replace(/\p{Mark}/gu, "")
      .replace(/[^\p{Letter}\p{Number}]+/gu, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 32) || "instance"
  );
}

/**
 * The alias for each instance, from its name, numbered when two share one.
 * Until an instance says its name, it keeps the alias it had last time.
 */
function aliases(): Map<string, string> {
  const s = store.get();
  const before = new Map<string, string>();
  for (const [alias, key] of saved) before.set(key, alias);
  const out = new Map<string, string>();
  const taken = new Set<string>();
  for (const key of s.order) {
    const name = s.instances[key]?.node?.name;
    const base = name ? ALIAS + slug(name) : (before.get(key) ?? `${ALIAS}instance`);
    let alias = base;
    for (let n = 2; taken.has(alias); n++) alias = `${base}-${n}`;
    taken.add(alias);
    out.set(key, alias);
    seen.set(alias, key);
    if (name) save(alias, key);
  }
  return out;
}

/** The first path segment, decoded, and the rest of the path. */
function split(pathname: string): [string, string] | null {
  const match = /^\/([^/]+)(.*)$/.exec(pathname);
  if (!match) return null;
  try {
    return [decodeURIComponent(match[1]!), match[2]!];
  } catch {
    return null;
  }
}

/** Router input: an alias in the address bar becomes the instance it stands for. */
export function aliasToKey(url: URL): URL | undefined {
  const parts = split(url.pathname);
  if (!parts || !parts[0].startsWith(ALIAS)) return undefined;
  aliases();
  const key = seen.get(parts[0]);
  if (!key) return undefined;
  url.pathname = `/${encodeURIComponent(key)}${parts[1]}`;
  return url;
}

/** Router output: in streamer mode, an instance's host becomes its alias. */
export function keyToAlias(url: URL): URL | undefined {
  if (!hidesPersonal()) return undefined;
  const parts = split(url.pathname);
  if (!parts) return undefined;
  const alias = aliases().get(parts[0]);
  if (!alias) return undefined;
  url.pathname = `/${alias}${parts[1]}`;
  return url;
}
