/**
 * Ephemeral messages (docs/ephemeral-messages.md): an agent's answer only
 * one person sees, for a while. The instance never stores them, so apps keep
 * them in memory, per place, until they run out or the person dismisses
 * them, and they're gone on reload. Pure, so node's test runner reads it as
 * is; fuwa/ephemeral.ts keeps the state.
 */

/** One kept message: when it was sent and when it stops showing (unix ms). */
export type Kept<M> = { readonly message: M; readonly at: number; readonly until: number };

/** Place -> the messages kept there, oldest first. Unchanged places keep their array. */
export type Ephemerals<M> = Readonly<Record<string, readonly Kept<M>[]>>;

type Identified = { readonly id: string };

/** A message arrived for `place`; the same id again replaces it. One already out of time is left out. */
export function added<M extends Identified>(state: Ephemerals<M>, place: string, kept: Kept<M>, now: number): Ephemerals<M> {
  if (kept.until <= now) return state;
  const others = (state[place] ?? []).filter((k) => k.message.id !== kept.message.id);
  // Usually the newest, so it goes at the end; otherwise in its place by time.
  const at = others.findIndex((k) => k.at > kept.at);
  const list = at === -1 ? [...others, kept] : [...others.slice(0, at), kept, ...others.slice(at)];
  return { ...state, [place]: list };
}

/** The person dismissed one; the same object when it wasn't there. */
export function dismissed<M extends Identified>(state: Ephemerals<M>, place: string, id: string): Ephemerals<M> {
  const list = state[place];
  if (!list?.some((k) => k.message.id === id)) return state;
  const left = list.filter((k) => k.message.id !== id);
  const next = { ...state };
  if (left.length) next[place] = left;
  else delete next[place];
  return next;
}

/** Drops whatever ran out by `now`; the same object when nothing did. */
export function expire<M>(state: Ephemerals<M>, now: number): Ephemerals<M> {
  let next: Record<string, readonly Kept<M>[]> | null = null;
  for (const [place, list] of Object.entries(state)) {
    const left = list.filter((k) => k.until > now);
    if (left.length === list.length) continue;
    next ??= { ...state };
    if (left.length) next[place] = left;
    else delete next[place];
  }
  return next ?? state;
}

/** When the next one runs out, for one timer; null with none kept. */
export function nextExpiry<M>(state: Ephemerals<M>): number | null {
  let soonest: number | null = null;
  for (const list of Object.values(state)) for (const k of list) if (soonest === null || k.until < soonest) soonest = k.until;
  return soonest;
}

/** Whole minutes left, rounded up; null under a minute ("Disappears in under a minute"). */
export function minutesLeft(until: number, now: number): number | null {
  const left = until - now;
  return left < 60_000 ? null : Math.ceil(left / 60_000);
}

/** How long until `minutesLeft` says something else, so a line can wait that long to redraw. */
export function nextTick(until: number, now: number): number {
  const left = until - now;
  if (left < 60_000) return Math.max(0, left);
  // At exactly a minute it's "under a minute" a moment later.
  if (left === 60_000) return 1;
  // Otherwise the count drops once the time left reaches the whole minute below.
  return left % 60_000 || 60_000;
}

/**
 * The channel's messages with the kept ones among them by time, each after
 * any message sent at the same moment. The same array when there are none.
 */
export function merged<M>(items: readonly M[], kept: readonly Kept<M>[], timeOf: (m: M) => number): readonly M[] {
  if (!kept.length) return items;
  const out: M[] = [];
  let k = 0;
  for (const m of items) {
    const t = timeOf(m);
    while (k < kept.length && kept[k].at < t) out.push(kept[k++].message);
    out.push(m);
  }
  while (k < kept.length) out.push(kept[k++].message);
  return out;
}
