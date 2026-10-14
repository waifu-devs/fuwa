/**
 * Typing indicators (docs/typing.md): who's typing where, and how the line
 * above the composer says it. Pure, so node's test runner reads it as is;
 * fuwa/typing.ts keeps the state and talks to the instance.
 *
 * A place is a channel, a thread's reply box in it, or a conversation. Each
 * place maps the people typing there to when they stop counting: a signal
 * keeps someone for TYPING_MS, and a message from them there ends it sooner.
 * Nothing here is ever kept or sent anywhere.
 */

/** How long one signal shows someone as typing. */
export const TYPING_MS = 10_000;
/** How often the composer says you're still typing, per place. */
export const SEND_EVERY_MS = 6_000;

/** Place -> user id -> when they stop showing (unix ms). Unchanged places keep their object. */
export type Typing = Readonly<Record<string, Readonly<Record<string, number>>>>;

/** A channel, or a thread's reply box in it (`threadId`, its parent message's id). */
export const channelPlace = (channelId: string, threadId = "") => (threadId ? `${channelId}#${threadId}` : channelId);
/** A direct-message conversation. */
export const dmPlace = (conversationId: string) => `dm:${conversationId}`;
/** Whether `place` is the channel or one of its threads. */
export const inChannel = (place: string, channelId: string) => place === channelId || place.startsWith(`${channelId}#`);

/**
 * Someone is typing in `place`, until `now + TYPING_MS`. You're never shown
 * to yourself. Someone already typing keeps their place in the line.
 */
export function started(state: Typing, place: string, userId: string, me: string | undefined, now: number): Typing {
  if (!userId || userId === me) return state;
  return { ...state, [place]: { ...state[place], [userId]: now + TYPING_MS } };
}

/** Someone stopped: their message arrived in `place`. */
export function stopped(state: Typing, place: string, userId: string): Typing {
  return stoppedWhere(state, (p) => p === place, userId);
}

/** Someone stopped in every place `where` picks (a secure channel's record doesn't say which thread). */
export function stoppedWhere(state: Typing, where: (place: string) => boolean, userId: string): Typing {
  let next: Record<string, Readonly<Record<string, number>>> | null = null;
  for (const [place, people] of Object.entries(state)) {
    if (!(userId in people) || !where(place)) continue;
    const { [userId]: _, ...rest } = people;
    next ??= { ...state };
    if (Object.keys(rest).length) next[place] = rest;
    else delete next[place];
  }
  return next ?? state;
}

/** Drops whoever ran out by `now`; the same object when nobody did. */
export function expire(state: Typing, now: number): Typing {
  let next: Record<string, Readonly<Record<string, number>>> | null = null;
  for (const [place, people] of Object.entries(state)) {
    const left = Object.entries(people).filter(([, until]) => until > now);
    if (left.length === Object.keys(people).length) continue;
    next ??= { ...state };
    if (left.length) next[place] = Object.fromEntries(left);
    else delete next[place];
  }
  return next ?? state;
}

/** When the next person runs out, for one timer; null with nobody typing. */
export function nextExpiry(state: Typing): number | null {
  let soonest: number | null = null;
  for (const people of Object.values(state)) for (const until of Object.values(people)) if (soonest === null || until < soonest) soonest = until;
  return soonest;
}

/** Who's typing in `place` at `now`, in the order they started. */
export function typers(state: Typing, place: string, now: number): string[] {
  return Object.entries(state[place] ?? {})
    .filter(([, until]) => until > now)
    .map(([id]) => id);
}

/** Which line to show: one, two or three names, or "several people" past that. */
export type TypingLabel = { kind: "one" | "two" | "three"; names: string[] } | { kind: "several"; names: [] } | null;

export function typingLabel(names: string[]): TypingLabel {
  switch (names.length) {
    case 0:
      return null;
    case 1:
      return { kind: "one", names };
    case 2:
      return { kind: "two", names };
    case 3:
      return { kind: "three", names };
    default:
      return { kind: "several", names: [] };
  }
}

/**
 * How often the composer signals: at most once every SEND_EVERY_MS per
 * place, and at once again after you send a message there.
 */
export function throttle(every = SEND_EVERY_MS) {
  const last = new Map<string, number>();
  return {
    /** Whether a signal for `place` goes out now; if it does, it counts as sent. */
    take(place: string, now: number): boolean {
      const before = last.get(place);
      if (before !== undefined && now - before < every) return false;
      last.set(place, now);
      return true;
    },
    /** A message went out from `place`: the next keystroke there signals again. */
    reset(place: string) {
      last.delete(place);
    },
  };
}
