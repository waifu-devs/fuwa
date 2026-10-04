import { Code } from "@connectrpc/connect";
import { Effect } from "effect";
import { dmEngine, DmError, type Content } from "@/e2ee/engine";
import { engine } from "./sync";
import { reportError, reportTiming, reportUsage } from "@/lib/reports";
import type { Voice as VoiceFile } from "@/e2ee/vault";
import type { Loader } from "@/voice/player";
import type { Clip } from "@/voice/recorder";
import { open, seal } from "@/voice/seal";
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
export async function sendDm(key: string, id: string, text: string, replyTo = 0) {
  reportUsage("dm.send");
  const nonce = crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`;
  setPending(key, id, (list) => [...list, { nonce, content: text, createdAt: Date.now(), failed: null }]);
  try {
    await ready(key).send(id, { text, replyTo });
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
  await sendDm(key, id, pending.content);
}

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

// ───────────────────────── Voice messages ─────────────────────────

/**
 * Recordings waiting to go, by pending nonce, so a failed one can be sent
 * again: with the file it already uploaded, if it got that far.
 */
type Outgoing = { clip: Clip; replyTo: number; uploaded?: VoiceFile };
const clips = new Map<string, Outgoing>();

/** Why a sealed upload's PUT failed, in words. */
async function put(key: string, uploadUrl: string, bytes: Uint8Array<ArrayBuffer>) {
  const path = URL.canParse(uploadUrl) ? new URL(uploadUrl).pathname : `/media/upload/${uploadUrl.slice(uploadUrl.lastIndexOf("/") + 1)}`;
  // Always to the instance's own address, whatever name it gave the link.
  const target = `${engine(key).url.replace(/\/+$/, "")}${path}`;
  const res = await fetch(target, { method: "PUT", body: bytes, credentials: "omit", referrerPolicy: "no-referrer" });
  if (!res.ok) throw new DmError((await res.text().catch(() => "")).trim() || "the voice message didn't upload");
}

/**
 * Sends a recording: sealed on this device under a key of its own, the
 * sealed bytes uploaded, and the key, waveform and length sent inside an
 * encrypted message. It shows at once, faded, like text.
 */
export async function sendVoiceDm(key: string, id: string, clip: Clip, replyTo = 0) {
  reportUsage("dm.voice");
  await sendVoice(key, id, { clip, replyTo });
}

async function sendVoice(key: string, id: string, out: Outgoing) {
  const { clip, replyTo } = out;
  const nonce = crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`;
  clips.set(nonce, out);
  setPending(key, id, (list) => [
    ...list,
    { nonce, content: "", createdAt: Date.now(), failed: null, voice: { durationMs: clip.durationMs, waveform: clip.waveform } },
  ]);
  const started = performance.now();
  try {
    // Sent again after a failure: the file it uploaded is still waiting for it.
    let voice = out.uploaded;
    if (!voice) {
      const sealed = await seal(clip.ogg);
      const reserved = await engine(key).api.dms.createSealedUpload(
        { conversationId: id, size: BigInt(sealed.bytes.length) },
        { timeoutMs: 20_000 },
      );
      await put(key, reserved.uploadUrl, sealed.bytes);
      voice = {
        mediaId: reserved.mediaId,
        key: sealed.key,
        sha256: sealed.sha256,
        size: sealed.bytes.length,
        durationMs: clip.durationMs,
        waveform: clip.waveform,
      };
      out.uploaded = voice;
      // This device already has the sound: no need to fetch it back to play it.
      keepOpened(reserved.mediaId, clip.ogg);
    }
    try {
      await ready(key).send(id, { voice, replyTo });
    } catch (err) {
      // Swept (it waited over a day) or otherwise gone: upload it anew next time.
      const code = err instanceof DmError ? null : toFuwaError(err).code;
      if (code === Code.NotFound || code === Code.InvalidArgument) out.uploaded = undefined;
      throw err;
    }
    reportTiming("dm.voice_send", performance.now() - started);
    clips.delete(nonce);
    setPending(key, id, (list) => list.filter((p) => p.nonce !== nonce));
  } catch (err) {
    reportError("voice_send", "dm.voice");
    const problem = dmProblem(err);
    setPending(key, id, (list) => list.map((p) => (p.nonce === nonce ? { ...p, failed: problem } : p)));
  }
}

/** Sends a voice message that failed again, or text if it was text. */
export async function retryPending(key: string, id: string, pending: PendingMessage) {
  const saved = clips.get(pending.nonce);
  if (!pending.voice) return retryDm(key, id, pending);
  dismissDm(key, id, pending.nonce);
  clips.delete(pending.nonce);
  if (saved) await sendVoice(key, id, saved);
}

/** Drops a message that didn't send, and its recording. */
export function dismissPending(key: string, id: string, nonce: string) {
  clips.delete(nonce);
  dismissDm(key, id, nonce);
}

/** Voice messages this device recorded, already open, by media id. */
const opened = new Map<string, Uint8Array<ArrayBuffer>>();
function keepOpened(mediaId: string, ogg: Uint8Array<ArrayBuffer>) {
  opened.set(mediaId, ogg);
  while (opened.size > 8) opened.delete(opened.keys().next().value!);
}

/**
 * Fetches a voice message's sealed bytes from this instance (by id, never a
 * link from the message) and opens them on this device.
 */
export function voiceLoader(key: string, voice: VoiceFile): Loader {
  return async () => {
    const mine = opened.get(voice.mediaId);
    if (mine) return mine;
    const started = performance.now();
    const res = await fetch(`${engine(key).url.replace(/\/+$/, "")}/media/${voice.mediaId}`, {
      credentials: "omit",
      referrerPolicy: "no-referrer",
    });
    if (res.status === 404) throw new Error("this voice message was deleted");
    if (!res.ok) throw new Error("this voice message couldn't be fetched");
    const bytes = new Uint8Array(await res.arrayBuffer());
    if (bytes.length !== voice.size) throw new Error("this voice message isn't the one that was sent");
    const ogg = await open(bytes, voice.key, voice.sha256);
    reportTiming("dm.voice_open", performance.now() - started);
    return ogg;
  };
}

/** The instance's caps on voice messages (0 for none), asked once in a while. */
const limits = new Map<string, { at: number; maxMs: number; maxBytes: number }>();
export async function voiceLimits(key: string): Promise<{ maxMs: number; maxBytes: number }> {
  const known = limits.get(key);
  if (known && Date.now() - known.at < 10 * 60_000) return known;
  try {
    const r = await engine(key).api.dms.getVoiceLimits({}, { timeoutMs: 10_000 });
    const next = { at: Date.now(), maxMs: Number(r.maxSeconds ?? 0n) * 1000, maxBytes: Number(r.maxBytes ?? 0n) };
    limits.set(key, next);
    return next;
  } catch {
    // An older instance, or offline: the server still checks what it can.
    return known ?? { maxMs: 0, maxBytes: 0 };
  }
}
