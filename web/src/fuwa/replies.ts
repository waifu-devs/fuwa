import { useSyncExternalStore } from "react";

/**
 * The message the next encrypted message answers, by draft: a conversation,
 * a secure channel, or `{id}#{parent}` for a secure channel's thread. It's
 * kept on this device and travels only inside the encryption, as the
 * message's `reply_to_sequence`; the instance never learns which line a
 * reply answers.
 */
const replying = new Map<string, number>();
const listeners = new Set<() => void>();

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

/** Starts answering the line at `seq` from a draft, or stops (0). */
export function setReplying(draft: string, seq: number) {
  if ((replying.get(draft) ?? 0) === seq) return;
  if (seq > 0) replying.set(draft, seq);
  else replying.delete(draft);
  for (const l of listeners) l();
}

export const clearReplying = (draft: string) => setReplying(draft, 0);

/** The line a draft answers, or 0. */
export const replyingIn = (draft: string) => replying.get(draft) ?? 0;

export function useReplying(draft: string): number {
  return useSyncExternalStore(subscribe, () => replying.get(draft) ?? 0);
}
