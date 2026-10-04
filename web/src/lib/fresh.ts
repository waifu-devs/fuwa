/**
 * Noticing that the instance serving this page now has a newer web app
 * (fuwa.chat redeploys on every merge). `components/UpdateReady.tsx` does
 * the asking and offers a reload; the page never reloads by itself. These
 * are the decisions, kept plain so they're tested.
 */

/** The app's entry script in an index.html, such as /assets/index-abc123.js. Its name changes with every build. */
export function entryOf(html: string): string | null {
  for (const tag of html.match(/<script\b[^>]*>/gi) ?? []) {
    if (!/\btype=["']module["']/i.test(tag)) continue;
    const src = /\bsrc=["']([^"']+)["']/i.exec(tag)?.[1];
    if (src) return src;
  }
  return null;
}

/** What's been seen of the newest build. */
export type Freshness = {
  /** The other build last seen, if any. */
  seen: string | null;
  /** How many looks in a row found it. */
  streak: number;
  /** A newer app is out, so this page is behind. */
  stale: boolean;
};

export const FRESH: Freshness = { seen: null, streak: 0, stale: false };

/**
 * Takes in one look at the instance's index.html. A different build has to
 * turn up twice in a row before the page counts as behind, so a deploy that
 * rolls out over a few minutes (one gateway new, another old) doesn't flap;
 * finding this page's own build again means there's nothing to do.
 */
export function look(state: Freshness, ours: string, found: string | null): Freshness {
  if (!found) return state;
  if (found === ours) return FRESH;
  const streak = found === state.seen ? state.streak + 1 : 1;
  return { seen: found, streak, stale: state.stale || streak >= 2 };
}
