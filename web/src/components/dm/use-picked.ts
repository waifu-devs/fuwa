import { useSyncExternalStore } from "react";
import { MAX_FILES } from "@/files/sealed";
import { cantSendFiles } from "@/fuwa/dms";
import { toast } from "@/lib/ui";

/**
 * Files picked for the next encrypted message, by conversation (or thread).
 * Nothing leaves this device until the message is sent: then each is sealed
 * here and uploaded as ciphertext.
 */
const NONE: File[] = [];
const picked = new Map<string, File[]>();
const listeners = new Set<() => void>();

/** Replaces the files picked for a draft. */
export function setPicked(draft: string, files: File[]) {
  if (files.length) picked.set(draft, files);
  else picked.delete(draft);
  for (const l of listeners) l();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export function usePicked(draft: string): File[] {
  return useSyncExternalStore(subscribe, () => picked.get(draft) ?? NONE);
}

/** Adds files to the next message, up to what one can carry. */
export function pickFiles(draft: string, files: File[]) {
  if (!files.length) return;
  const next = [...(picked.get(draft) ?? NONE), ...files];
  const problem = cantSendFiles(next);
  if (problem) toast(problem);
  setPicked(draft, next.slice(0, MAX_FILES).filter((f) => !cantSendFiles([f])));
}

export const clearPicked = (draft: string) => setPicked(draft, []);
