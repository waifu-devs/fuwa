import { useSyncExternalStore } from "react";
import type { DirectMessageEvent, DmPin } from "@/gen/fuwa/v1/dm_pb";
import type { Event, Message } from "@/gen/fuwa/v1/types_pb";
import { reportUsage } from "@/lib/reports";
import { toFuwaError } from "./errors";
import { applyEvent, updateInstance, withSharedAuthors, withUsers } from "./store";
import { engine } from "./sync";

/*
 * Pinned messages: a channel's (or a thread's) and a direct-message
 * conversation's, each list loaded when someone looks at it and kept
 * current by the events that say a pin changed. Kept apart from the main
 * store, which only marks pinned messages (Message.pinnedAt); a direct
 * message's pins name records by their place, and each app draws them from
 * what it opened itself.
 */

export type ChannelPins = { status: "loading" | "ready" | "failed"; messages: Message[]; hasMore: boolean };
export type DmPins = { status: "loading" | "ready" | "failed"; pins: DmPin[]; hasMore: boolean };

const PAGE = 50;

const lists = new Map<string, ChannelPins | DmPins>();
const listeners = new Set<() => void>();
const notify = () => {
  for (const l of listeners) l();
};
const subscribe = (l: () => void) => (listeners.add(l), () => listeners.delete(l));

const channelKey = (instanceKey: string, channelId: string, threadId = "") => `${instanceKey}|c|${channelId}|${threadId}`;
const dmKey = (instanceKey: string, conversationId: string) => `${instanceKey}|d|${conversationId}`;

function put(key: string, value: ChannelPins | DmPins) {
  lists.set(key, value);
  notify();
}

/** A channel's (or thread's) pins, once loaded. */
export function useChannelPins(instanceKey: string, channelId: string, threadId = ""): ChannelPins | undefined {
  return useSyncExternalStore(subscribe, () => lists.get(channelKey(instanceKey, channelId, threadId)) as ChannelPins | undefined);
}

/** A conversation's pins, once loaded. */
export function useDmPins(instanceKey: string, conversationId: string): DmPins | undefined {
  return useSyncExternalStore(subscribe, () => lists.get(dmKey(instanceKey, conversationId)) as DmPins | undefined);
}

/** Loads a channel's pins, the latest first; `more` adds the next page. */
export async function loadChannelPins(instanceKey: string, serverId: string, channelId: string, threadId = "", more = false) {
  const key = channelKey(instanceKey, channelId, threadId);
  const had = lists.get(key) as ChannelPins | undefined;
  const afterId = more ? (had?.messages.at(-1)?.id ?? "") : "";
  if (!more) put(key, { status: "loading", messages: had?.messages ?? [], hasMore: had?.hasMore ?? false });
  try {
    const res = await engine(instanceKey).api.messages.listPins({ serverId, channelId, threadId, limit: PAGE, afterId });
    updateInstance(instanceKey, (i) => ({ ...i, users: withSharedAuthors(withUsers(i.users, res.authors), res.messages) }));
    const messages = more ? [...(had?.messages ?? []), ...res.messages.filter((m) => !had?.messages.some((h) => h.id === m.id))] : res.messages;
    put(key, { status: "ready", messages, hasMore: res.hasMore });
  } catch {
    put(key, { status: "failed", messages: had?.messages ?? [], hasMore: false });
  }
}

/** Pins a message in a channel or thread, or unpins it; the store marks it at once. */
export async function pinMessage(instanceKey: string, serverId: string, channelId: string, messageId: string, pinned: boolean) {
  reportUsage(pinned ? "message.pin" : "message.unpin");
  try {
    const { message } = await engine(instanceKey).api.messages.pinMessage({ serverId, channelId, messageId, pinned });
    if (message) markPinned(instanceKey, serverId, message);
  } catch (err) {
    throw toFuwaError(err);
  }
}

/** Marks a message pinned or not in the store, as the server's event will. */
function markPinned(instanceKey: string, serverId: string, message: Message) {
  updateInstance(instanceKey, (i) =>
    applyEvent(
      i,
      {
        serverId,
        payload: {
          case: "messagePinned",
          value: { channelId: message.channelId, messageId: message.id, threadId: message.threadId, pinnedAt: message.pinnedAt },
        },
      } as Event,
      null,
    ),
  );
  refreshLoaded(instanceKey, serverId, message.channelId, message.threadId, message.alsoInChannel);
}

/** Loads again the pin lists a change shows in, where someone has them open. */
function refreshLoaded(instanceKey: string, serverId: string, channelId: string, threadId: string, alsoInChannel: boolean) {
  const keys = threadId && !alsoInChannel ? [threadId] : [""];
  for (const t of keys) {
    if (lists.has(channelKey(instanceKey, channelId, t))) void loadChannelPins(instanceKey, serverId, channelId, t);
  }
}

/** A server's event: a pin that changed, or a message that went, reloads the lists that show it. */
export function onPinEvent(instanceKey: string, event: Event) {
  const p = event.payload;
  if (p.case === "messagePinned") {
    const { channelId, threadId } = p.value;
    for (const t of new Set([threadId, ""])) {
      if (lists.has(channelKey(instanceKey, channelId, t))) void loadChannelPins(instanceKey, event.serverId, channelId, t);
    }
  } else if (p.case === "messageDeleted") {
    for (const [key, list] of lists) {
      if (!key.startsWith(`${instanceKey}|c|${p.value.channelId}|`) || !("messages" in list)) continue;
      const messages = list.messages.filter((m) => m.id !== p.value.messageId);
      if (messages.length !== list.messages.length) put(key, { ...list, messages });
    }
  }
}

/** Loads a conversation's pins, the latest first; `more` adds the next page. */
export async function loadDmPins(instanceKey: string, conversationId: string, more = false) {
  const key = dmKey(instanceKey, conversationId);
  const had = lists.get(key) as DmPins | undefined;
  if (!had) put(key, { status: "loading", pins: [], hasMore: false });
  const afterSequence = more ? had?.pins.at(-1)?.sequence : undefined;
  try {
    const res = await engine(instanceKey).api.dms.listRecordPins({ conversationId, limit: PAGE, afterSequence });
    const pins = more ? [...(had?.pins ?? []), ...res.pins.filter((p) => !had?.pins.some((h) => h.sequence === p.sequence))] : res.pins;
    put(key, { status: "ready", pins, hasMore: res.hasMore });
  } catch {
    put(key, { status: "failed", pins: had?.pins ?? [], hasMore: had?.hasMore ?? false });
  }
}

/** Pins a direct message by its place in the conversation, or unpins it. */
export async function pinDm(instanceKey: string, conversationId: string, sequence: number, pinned: boolean) {
  reportUsage(pinned ? "message.pin" : "message.unpin");
  try {
    const { pin } = await engine(instanceKey).api.dms.pinRecord({ conversationId, sequence: BigInt(sequence), pinned });
    changeDm(instanceKey, conversationId, BigInt(sequence), pin);
  } catch (err) {
    throw toFuwaError(err);
  }
}

function changeDm(instanceKey: string, conversationId: string, sequence: bigint, pin: DmPin | undefined) {
  const key = dmKey(instanceKey, conversationId);
  const list = lists.get(key) as DmPins | undefined;
  if (!list) return;
  const rest = list.pins.filter((p) => p.sequence !== sequence);
  put(key, { ...list, pins: pin ? [pin, ...rest] : rest });
}

/** A direct-message event: a pin that changed, or a record that was deleted (its pin goes with it). */
export function onDmPinEvent(instanceKey: string, event: DirectMessageEvent) {
  const p = event.payload;
  if (p.case === "pinUpdated" && p.value.pin) {
    const pin = p.value.pin;
    changeDm(instanceKey, pin.conversationId, pin.sequence, p.value.pinned ? pin : undefined);
  } else if (p.case === "recordDeleted") {
    changeDm(instanceKey, p.value.conversationId, p.value.sequence, undefined);
  }
}

/** Forgets an instance's lists, when it's removed or signed out of. */
export function forgetPins(instanceKey: string) {
  for (const key of [...lists.keys()]) if (key.startsWith(`${instanceKey}|`)) lists.delete(key);
  notify();
}

// ── Jumping to a pinned direct message ────────────────────────────────────

type DmJump = { conversationId: string; seq: number; at: number };
let dmJump: DmJump | null = null;

/** Asks a conversation's list to bring a message into view. */
export function requestDmJump(conversationId: string, seq: number) {
  dmJump = { conversationId, seq, at: Date.now() };
  notify();
}

/** The jump waiting for a conversation, if any. */
export function useDmJump(conversationId: string): DmJump | null {
  return useSyncExternalStore(subscribe, () => (dmJump?.conversationId === conversationId ? dmJump : null));
}

/** The list took the jump. */
export function doneDmJump(jump: DmJump) {
  if (dmJump === jump) {
    dmJump = null;
    notify();
  }
}
