import { useCallback, useSyncExternalStore } from "react";
import type { Event, Message } from "@/gen/fuwa/v1/types_pb";
import { added, dismissed, type Ephemerals, expire, type Kept, nextExpiry } from "@/lib/ephemeral";
import { toDate } from "@/lib/format";
import { channelPlace } from "@/lib/typing";
import { store } from "./store";

/**
 * Ephemeral messages (docs/ephemeral-messages.md), per instance: an agent's
 * answers only you see, kept in memory until they run out or you dismiss
 * them. Kept apart from the main store like typing: they move no sequence,
 * aren't news (no unread, no notification, no read mark) and are never
 * written down, so a reload forgets them.
 */

// One Ephemerals for every instance, its places named "<instance>\n<channel>[#<thread>]".
let state: Ephemerals<Message> = {};
const listeners = new Map<string, Set<() => void>>();
let timer: ReturnType<typeof setTimeout> | null = null;
const NONE: readonly Kept<Message>[] = [];

const full = (key: string, channelId: string, threadId = "") => `${key}\n${channelPlace(channelId, threadId)}`;

function set(next: Ephemerals<Message>) {
  if (next === state) return;
  const before = state;
  state = next;
  for (const [place, fns] of listeners) if (before[place] !== next[place]) for (const fn of fns) fn();
  schedule();
}

/** One timer, for whichever runs out first. */
function schedule() {
  if (timer) clearTimeout(timer);
  timer = null;
  const at = nextExpiry(state);
  if (at !== null) timer = setTimeout(() => set(expire(state, Date.now())), Math.max(0, at - Date.now()));
}

/**
 * A server event: keeps an ephemeral message sent to you. True when the
 * event was one, so the caller takes it no further.
 */
export function onEphemeralEvent(key: string, event: Event): boolean {
  if (event.payload.case !== "ephemeralMessageCreated") return false;
  const m = event.payload.value.message;
  // Only ever sent to the one person; anything else (or without an end) isn't shown.
  if (m?.ephemeral && m.ephemeral.userId === store.get().instances[key]?.me?.id) {
    const kept = { message: m, at: toDate(m.createdAt).getTime(), until: toDate(m.ephemeral.expiresAt).getTime() };
    set(added(state, full(key, m.channelId, m.threadId), kept, Date.now()));
  }
  return true;
}

/** Takes one off this device, before it runs out. */
export function dismissEphemeral(key: string, channelId: string, threadId: string, id: string) {
  set(dismissed(state, full(key, channelId, threadId), id));
}

/** The ephemeral messages kept for a channel (or a thread in it), oldest first; the same array until that changes. */
export function useEphemeralIn(key: string, channelId: string, threadId = ""): readonly Kept<Message>[] {
  const place = full(key, channelId, threadId);
  const subscribe = useCallback(
    (fn: () => void) => {
      let fns = listeners.get(place);
      if (!fns) listeners.set(place, (fns = new Set()));
      fns.add(fn);
      return () => {
        fns.delete(fn);
        if (!fns.size) listeners.delete(place);
      };
    },
    [place],
  );
  return useSyncExternalStore(subscribe, () => state[place] ?? NONE);
}
