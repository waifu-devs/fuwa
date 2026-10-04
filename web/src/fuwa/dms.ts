import { Code } from "@connectrpc/connect";
import { Effect } from "effect";
import { dmEngine, DmError, type Content } from "@/e2ee/engine";
import { engine } from "./sync";
import { reportUsage } from "@/lib/reports";
import { call, FuwaError, toFuwaError } from "./errors";
import { store, updateDms, type PendingMessage } from "./store";

/**
 * What people do with encrypted direct messages. The encryption happens in
 * this browser (`@/e2ee/engine`); these fold it into the store, with the same
 * optimistic sending as channels.
 */

const ready = (key: string) => {
  const dms = dmEngine(key);
  if (!dms) throw new DmError(store.get().instances[key]?.dms.problem ?? "encrypted messages are still starting");
  return dms;
};

/** In words, for the screen. */
export const dmProblem = (err: unknown) => (err instanceof DmError ? err.message : toFuwaError(err).message);

/** The conversation with someone: the one you have, or a new one. Answers its id. */
export const openConversation = (key: string, userId: string) =>
  Effect.gen(function* () {
    const { conversation } = yield* call((signal) => engine(key).api.dms.openConversation({ userId }, { signal }));
    if (!conversation) return yield* Effect.fail(new FuwaError({ code: Code.Unknown, message: "couldn't open that conversation" }));
    dmEngine(key)?.add(conversation);
    return conversation.id;
  });

/** Catches the conversation up and adds everyone's devices, so it's ready to write in. */
export async function prepareConversation(key: string, id: string) {
  try {
    await ready(key).prepare(id);
    updateDms(key, (d) => (d.blocked[id] ? { ...d, blocked: { ...d.blocked, [id]: "" } } : d));
  } catch (err) {
    if (err instanceof DmError) updateDms(key, (d) => ({ ...d, blocked: { ...d.blocked, [id]: err.message } }));
    else throw err;
  }
}

const setPending = (key: string, id: string, fn: (list: PendingMessage[]) => PendingMessage[]) =>
  updateDms(key, (d) => ({ ...d, pending: { ...d.pending, [id]: fn(d.pending[id] ?? []) } }));

/** Sends a message. It shows at once, faded, until the instance has it; failing leaves it with a retry. */
/** Where a message goes in a secure channel: into a thread (and maybe the channel too). */
export type ThreadTarget = { thread: number; inChannel: boolean };

export async function sendDm(key: string, id: string, text: string, target?: ThreadTarget) {
  reportUsage(target ? "e2ee.thread_reply" : "dm.send");
  const nonce = crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`;
  setPending(key, id, (list) => [...list, { nonce, content: text, createdAt: Date.now(), failed: null, ...target }]);
  try {
    await ready(key).send(id, { text, ...target });
    setPending(key, id, (list) => list.filter((p) => p.nonce !== nonce));
    updateDms(key, (d) => (d.blocked[id] ? { ...d, blocked: { ...d.blocked, [id]: "" } } : d));
  } catch (err) {
    const problem = dmProblem(err);
    setPending(key, id, (list) => list.map((p) => (p.nonce === nonce ? { ...p, failed: problem } : p)));
    if (err instanceof DmError) updateDms(key, (d) => ({ ...d, blocked: { ...d.blocked, [id]: problem } }));
  }
}

export const dismissDm = (key: string, id: string, nonce: string) => setPending(key, id, (list) => list.filter((p) => p.nonce !== nonce));

export async function retryDm(key: string, id: string, pending: PendingMessage) {
  dismissDm(key, id, pending.nonce);
  await sendDm(key, id, pending.content, pending.thread ? { thread: pending.thread, inChannel: !!pending.inChannel } : undefined);
}

/** Locks or unlocks a secure channel's thread: a signed line only devices read, counted from people with Manage Messages. */
export const lockSecureThread = (key: string, id: string, parent: number, locked: boolean) => ready(key).send(id, { lock: parent, locked });

export const followSecureThread = (key: string, id: string, parent: number, on: boolean) => ready(key).followThread(id, parent, on);

export const markSecureThreadRead = (key: string, id: string, parent: number, seq: number) =>
  dmEngine(key)?.markThreadRead(id, parent, seq).catch(() => {});

/** New text for one of your messages, sent encrypted like a message. */
export const editDm = (key: string, id: string, seq: number, text: string) => ready(key).send(id, { edit: seq, text } satisfies Content);

export const deleteDm = (key: string, id: string, seq: number) => ready(key).remove(id, seq);

export const markDmRead = (key: string, id: string) => dmEngine(key)?.markRead(id).catch(() => {});

/** Marks the conversation's safety number as checked with the other person, or not ("" ). */
export const verifyDm = (key: string, id: string, safety: string) => ready(key).verify(id, safety);

/**
 * Starts following a secure channel: catches up on it. Someone who may write
 * there also gets it ready to write in, bringing in everyone who can see it;
 * someone who only reads joins by themselves and changes nothing.
 */
export async function prepareSecureChannel(key: string, serverId: string, channelId: string, write: boolean) {
  const dms = ready(key);
  const following = dms.followChannel(serverId, channelId);
  if (write) await prepareConversation(key, channelId);
  else await following;
}

/** Starts a secure channel's encryption over (Manage Channels), then gets it going again if you may write there. */
export async function resetSecureChannel(key: string, serverId: string, channelId: string, write: boolean) {
  reportUsage("secure.reset");
  await engine(key).api.secure.resetSecureChannel({ serverId, channelId });
  await ready(key).followChannel(serverId, channelId);
  if (write) await prepareConversation(key, channelId);
}

/** Turns passing earlier messages on to people added later on or off for a secure channel (Manage Channels). */
export async function setSecureHistory(key: string, serverId: string, channelId: string, shareHistory: boolean) {
  reportUsage(shareHistory ? "secure.history_on" : "secure.history_off");
  await engine(key).api.secure.setSecureHistory({ serverId, channelId, shareHistory });
  updateDms(key, (d) => ({ ...d, secureHistory: { ...d.secureHistory, [channelId]: shareHistory } }));
  void dmEngine(key)?.followChannel(serverId, channelId);
}
