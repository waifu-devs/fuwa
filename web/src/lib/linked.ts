/**
 * Signing in with waifu.dev (a linked account). The app asks the instance to
 * start a sign-in, keeps a secret only it knows while the browser visits
 * waifu.dev, and finishes on the way back at /auth/waifu/callback. When this
 * app signs in to an instance at another address, waifu.dev sends the browser
 * to that instance first, whose callback page hands the code back here.
 */

/** Where waifu.dev sends the browser back to, on every fuwa app. */
export const CALLBACK = "/auth/waifu/callback";

/** The issuer fuwa instances use unless their admin picks another. */
export const WAIFU_DEV_ISSUER = "https://api.waifu.dev";

/** A sign-in waiting for the browser to come back. */
export type PendingSignIn = {
  /** The instance being signed in to. */
  url: string;
  /** Proves this app started it. */
  secret: string;
  /** Where to go once signed in, or the instance's home. */
  next: string | null;
  startedAt: number;
};

const KEY = "fuwa.linked.";
/** Sign-ins run out on the instance after ten minutes. */
const TTL = 10 * 60 * 1000;

export function savePending(state: string, pending: PendingSignIn) {
  try {
    sessionStorage.setItem(KEY + state, JSON.stringify(pending));
    return true;
  } catch {
    return false;
  }
}

/** The sign-in this tab started with `state`, taken so it finishes once. */
export function takePending(state: string): PendingSignIn | null {
  try {
    const raw = sessionStorage.getItem(KEY + state);
    sessionStorage.removeItem(KEY + state);
    const pending = raw ? (JSON.parse(raw) as PendingSignIn) : null;
    return pending && Date.now() - pending.startedAt < TTL ? pending : null;
  } catch {
    return null;
  }
}

/** 32 random bytes, URL-safe base64. */
export function newSecret() {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export async function sha256Hex(text: string) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** Whether waifu.dev can send people back to this address: https, or this machine while testing. */
export function canReturnTo(url: string) {
  try {
    const u = new URL(url.trim());
    return u.protocol === "https:" || (u.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(u.hostname));
  } catch {
    return false;
  }
}

/** "waifu.dev" for the default issuer, otherwise the issuer's host. */
export function issuerName(issuer: string | undefined) {
  if (!issuer || issuer === WAIFU_DEV_ISSUER) return "waifu.dev";
  try {
    return new URL(issuer).host;
  } catch {
    return issuer;
  }
}
