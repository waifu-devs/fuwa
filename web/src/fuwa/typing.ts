import { useCallback, useSyncExternalStore } from "react";
import type { DirectMessageEvent } from "@/gen/fuwa/v1/dm_pb";
import { SecureRecordKind } from "@/gen/fuwa/v1/secure_pb";
import type { Event } from "@/gen/fuwa/v1/types_pb";
import { instanceHas } from "@/lib/compat";
import { getPrefs } from "@/lib/prefs";
import { channelPlace, dmPlace, expire, inChannel, nextExpiry, started, stopped, stoppedWhere, throttle, type Typing } from "@/lib/typing";
import { store } from "./store";
import { engine } from "./sync";

/**
 * Typing indicators (docs/typing.md), per instance: who's typing where, and
 * telling the instance you are.
 *
 * Kept apart from the main store on purpose, like presence: signals come
 * every few seconds while people type, and each line above a composer
 * subscribes to its one place, so a signal redraws that line and nothing
 * else. Never written down, reported or logged.
 */

/** Where you type: a server channel (or a thread's reply box in it), or a conversation. */
export type TypingTarget = { serverId: string; channelId: string; threadId?: string } | { conversationId: string };

/** The place a target types in, as lib/typing.ts names it. */
export const placeOf = (target: TypingTarget) =>
  "conversationId" in target ? dmPlace(target.conversationId) : channelPlace(target.channelId, target.threadId);

// One Typing for every instance, its places named "<instance>\n<place>".
let state: Typing = {};
const listeners = new Map<string, Set<() => void>>();
let timer: ReturnType<typeof setTimeout> | null = null;
const NOBODY: Readonly<Record<string, number>> = {};

const full = (key: string, place: string) => `${key}\n${place}`;

function set(next: Typing) {
  if (next === state) return;
  const before = state;
  state = next;
  for (const [place, fns] of listeners) if (before[place] !== next[place]) for (const fn of fns) fn();
  schedule();
}

/** One timer, for whoever runs out first. */
function schedule() {
  if (timer) clearTimeout(timer);
  timer = null;
  const at = nextExpiry(state);
  if (at !== null) timer = setTimeout(() => set(expire(state, Date.now())), Math.max(0, at - Date.now()));
}

const me = (key: string) => store.get().instances[key]?.me?.id;

/**
 * A server event: someone typing, or a message that ends someone's typing.
 * Typing itself moves no sequence and touches nothing in the main store.
 */
export function onTypingEvent(key: string, event: Event) {
  const p = event.payload;
  switch (p.case) {
    case "typingStarted":
      set(started(state, full(key, channelPlace(p.value.channelId, p.value.threadId)), p.value.userId, me(key), Date.now()));
      return;
    case "messageCreated": {
      const m = p.value.message;
      if (m) set(stopped(state, full(key, channelPlace(m.channelId, m.threadId)), m.authorId));
      return;
    }
    case "secureRecordAdded": {
      // A secure message doesn't say which thread it's in, so it ends their typing anywhere in the channel.
      const r = p.value.record;
      if (r?.kind === SecureRecordKind.MESSAGE) {
        const prefix = `${key}\n`;
        set(stoppedWhere(state, (place) => place.startsWith(prefix) && inChannel(place.slice(prefix.length), r.channelId), r.senderId));
      }
      return;
    }
  }
}

/** A direct-message event: the other side typing, or a record from them. */
export function onDmTypingEvent(key: string, event: DirectMessageEvent) {
  const p = event.payload;
  if (p.case === "typing") set(started(state, full(key, dmPlace(p.value.conversationId)), p.value.userId, me(key), Date.now()));
  else if (p.case === "recordAdded") set(stopped(state, full(key, dmPlace(p.value.conversationId)), p.value.senderId));
}

/** Who's typing in one place, in the order they started; the same object until that changes. */
export function useTypingIn(key: string, target: TypingTarget): Readonly<Record<string, number>> {
  const place = full(key, placeOf(target));
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
  return useSyncExternalStore(subscribe, () => state[place] ?? NOBODY);
}

const sending = throttle();

/**
 * You typed in a composer: tells the instance, at most every few seconds
 * per place, unless the Show when I'm typing setting is off or the
 * instance doesn't have typing. Fire and forget: a failure says nothing.
 */
export function typed(key: string, target: TypingTarget) {
  if (!getPrefs().sendTyping) return;
  if (!instanceHas(store.get().instances[key]?.node?.versions, "typing")) return;
  if (!sending.take(full(key, placeOf(target)), Date.now())) return;
  try {
    const api = engine(key).api;
    const call =
      "conversationId" in target
        ? api.dms.sendConversationTyping({ conversationId: target.conversationId })
        : api.messages.sendTyping({ serverId: target.serverId, channelId: target.channelId, threadId: target.threadId ?? "" });
    call.catch(() => {});
  } catch {
    // The instance went away meanwhile.
  }
}

/** You sent a message there: typing again right after says so at once. */
export function sentFrom(key: string, target: TypingTarget) {
  sending.reset(full(key, placeOf(target)));
}
