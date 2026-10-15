import { create } from "@bufbuild/protobuf";
import { ReadMarkSchema, type ReadMark } from "@/gen/fuwa/v1/account_pb";
import type { ReadMarks } from "@/gen/fuwa/v1/live_pb";
import { ChannelType, type Message } from "@/gen/fuwa/v1/types_pb";
import { dmEngine } from "@/e2ee/engine";
import { instanceHas } from "@/lib/compat";
import { byMessage, newestMark, withReadMarks } from "./read-marks";
import { store, updateInstance } from "./store";
import { engine } from "./sync";

/**
 * Read marks on every device (docs/read-marks.md): what you read here goes
 * to the instance, which passes it to your other devices' live connections,
 * and what you read there clears unread counts here. Channels and threads
 * are marked "up to now" (the instance fills in the id); conversations and
 * secure channels by the last record this device noted it read.
 */

/** How long marks wait to go out together, so a busy channel on screen sends one now and then, not one per message. */
const SEND_AFTER_MS = 1500;
/** As many as one MarkRead takes. */
const PER_CALL = 100;

/** Per instance, the marks waiting to go out, by place. */
const queued = new Map<string, Map<string, bigint>>();
const timers = new Map<string, ReturnType<typeof setTimeout>>();

/** Whether you're looking at the app (not another tab or window). */
const looking = () => typeof document === "undefined" || document.visibilityState === "visible";

const marksHere = (key: string) => {
  const i = store.get().instances[key];
  return !!i?.me && instanceHas(i.node?.versions, "read-marks");
};

function queue(key: string, place: string, sequence: bigint) {
  if (!place || !marksHere(key)) return;
  const places = queued.get(key) ?? new Map<string, bigint>();
  queued.set(key, places);
  if ((places.get(place) ?? -1n) < sequence) places.set(place, sequence);
  if (!timers.has(key)) timers.set(key, setTimeout(() => void send(key), SEND_AFTER_MS));
}

async function send(key: string) {
  timers.delete(key);
  const places = queued.get(key);
  queued.delete(key);
  if (!places?.size || !marksHere(key)) return;
  const marks = [...places].map(([place, sequence]) => create(ReadMarkSchema, { place, sequence }));
  for (let n = 0; n < marks.length; n += PER_CALL) {
    try {
      const res = await engine(key).api.account.markRead({ marks: marks.slice(n, n + PER_CALL) });
      // As kept: the id the instance filled in, so what came before stops counting here too.
      updateInstance(key, (i) => withReadMarks(i, res.marks));
    } catch {
      // Not passed on this time; the next read there will be.
    }
  }
}

/** You read a channel or a thread (by the id of the message it's under) up to now, if you're looking at it. */
export function noteRead(key: string, place: string) {
  if (looking()) queue(key, place, 0n);
}

/** You marked channels read by hand (Mark as read): up to now, wherever they are. */
export function noteMarkedRead(key: string, channelIds: readonly string[]) {
  for (const id of channelIds) queue(key, id, 0n);
}

/** This device noted reading a conversation or secure channel (or a thread there, "<id>/<record>") up to a record. */
export function noteRecordRead(key: string, place: string, sequence: number) {
  if (sequence > 0) queue(key, place, BigInt(sequence));
}

/** The channel and thread on screen, read up to now: on focus, on coming back to the app, as messages arrive in them. */
export function noteFocusRead() {
  const focus = store.get().focus;
  const inst = focus && store.get().instances[focus.instance];
  if (!focus || !inst) return;
  // A server's channel (conversations and secure channels note their own).
  const channel = Object.values(inst.channels)
    .flat()
    .find((c) => c.id === focus.channel);
  if (!channel || channel.type === ChannelType.SECURE) return;
  noteRead(focus.instance, channel.id);
  if (focus.thread) noteRead(focus.instance, focus.thread);
}

/** A message came live: in the channel or thread on screen, that's read too. */
export function noteArrived(key: string, message: Message | undefined) {
  const focus = store.get().focus;
  if (!message || focus?.instance !== key || message.authorId === store.get().instances[key]?.me?.id) return;
  if (message.channelId === focus.channel || (!!message.threadId && message.threadId === focus.thread)) noteFocusRead();
}

/** Marks heard on the live connection: from every device of yours, this one's too. Returns where to ask from next time. */
export function takeReadMarks(key: string, reads: ReadMarks, since: bigint): bigint {
  updateInstance(key, (i) => withReadMarks(i, reads.marks));
  const records = reads.marks.filter((m: ReadMark) => !byMessage(m) && m.sequence > 0n);
  if (records.length) {
    const dms = dmEngine(key);
    for (const m of records) void dms?.readElsewhere(m.place, Number(m.sequence)).catch(() => {});
  }
  return newestMark(reads.marks, since);
}

if (typeof document !== "undefined") document.addEventListener("visibilitychange", () => looking() && noteFocusRead());
