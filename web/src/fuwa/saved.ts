/**
 * The instances this browser knows about, and a session token for each one
 * the person signed in to. They live in localStorage and never leave the
 * browser, except that each token goes to its own instance.
 */

const KEY = "fuwa:instances:v1";

export type SavedInstance = { url: string; token: string | null };

export function loadSaved(): SavedInstance[] {
  try {
    const raw = localStorage.getItem(KEY);
    const list = raw ? (JSON.parse(raw) as unknown) : [];
    if (!Array.isArray(list)) return [];
    return list.filter(
      (i): i is SavedInstance => typeof i?.url === "string" && (i.token === null || typeof i.token === "string"),
    );
  } catch {
    return [];
  }
}

export function storeSaved(list: SavedInstance[]) {
  try {
    localStorage.setItem(KEY, JSON.stringify(list));
  } catch {
    // Private mode or storage full: the session still works until the tab closes.
  }
}

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
