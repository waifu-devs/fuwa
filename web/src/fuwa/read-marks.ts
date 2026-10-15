import type { ReadMark } from "@/gen/fuwa/v1/account_pb";
import type { InstanceState } from "./store";

/**
 * The pure half of read marks (docs/read-marks.md): how far you've read in a
 * channel or thread, heard from any of your devices, clears its unread count
 * here and keeps what came before it from counting again. Conversations and
 * secure channels go by sequence and are the encryption engine's (their
 * counts come from what each device notes it read). `reads.ts` sends and
 * takes the marks. Only types come from elsewhere, so node's test runner
 * reads this file as is.
 */

/** Whether a mark is for a channel or thread (a message id), not a conversation (a sequence). */
export const byMessage = (mark: Pick<ReadMark, "messageId" | "sequence">) => !!mark.messageId && !mark.sequence;

/** Channels' and threads' marks, as kept here: their unread counts go, and what they cover won't count again. */
export function withReadMarks(i: InstanceState, marks: readonly ReadMark[]): InstanceState {
  let { readMarks, unread, threadUnread } = i;
  for (const mark of marks) {
    if (!byMessage(mark)) continue;
    if ((readMarks[mark.place] ?? "") < mark.messageId) readMarks = { ...readMarks, [mark.place]: mark.messageId };
    // A place is a channel or a thread (the message it's under), never both.
    if (unread[mark.place]) {
      const { [mark.place]: _, ...rest } = unread;
      unread = rest;
    }
    if (threadUnread[mark.place]) {
      const { [mark.place]: _, ...rest } = threadUnread;
      threadUnread = rest;
    }
  }
  if (readMarks === i.readMarks && unread === i.unread && threadUnread === i.threadUnread) return i;
  return { ...i, readMarks, unread, threadUnread };
}

/** Whether a message in a channel or thread was read already, on this device or another. */
export const readAlready = (i: InstanceState, place: string, messageId: string) => messageId <= (i.readMarks[place] ?? "");

/** The newest `atMs` among marks, or `since` when none is newer: where a connection asks for marks from next time. */
export function newestMark(marks: readonly ReadMark[], since: bigint): bigint {
  let at = since;
  for (const mark of marks) if (mark.atMs > at) at = mark.atMs;
  return at;
}
