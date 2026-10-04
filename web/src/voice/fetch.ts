/*
 * Fetching a voice message's file, wherever it's kept: never more than a
 * long voice message can be, nor more than the message says it is.
 */
import { shownPicture } from "@/lib/shown";
import type { Loader } from "./player";

/**
 * The biggest voice message this app fetches: fifteen minutes at 64 kbps,
 * padded, which is twice what this app records at.
 */
export const MAX_VOICE_BYTES = 8 * 1024 * 1024;

/** Reads exactly `size` bytes, stopping as soon as there are more. */
export async function readExactly(body: ReadableStream<Uint8Array>, size: number): Promise<Uint8Array<ArrayBuffer>> {
  const out = new Uint8Array(size);
  const reader = body.getReader();
  let at = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (at + value.length > size) throw new Error("this voice message isn't the one that was sent");
      out.set(value, at);
      at += value.length;
    }
  } finally {
    void reader.cancel().catch(() => {});
  }
  if (at !== size) throw new Error("this voice message isn't the one that was sent");
  return out;
}

/**
 * Fetches a voice message attached in a server channel: only from a fuwa
 * instance, never bigger than the message says it is, without cookies or
 * a referrer. The player plays it from memory.
 */
export function attachmentLoader(url: string, size: number): Loader {
  return async () => {
    const from = shownPicture(url);
    if (!from) throw new Error("this voice message isn't on a fuwa instance");
    if (!(size > 0) || size > MAX_VOICE_BYTES) throw new Error("this voice message is too big to play here");
    const res = await fetch(from, { credentials: "omit", referrerPolicy: "no-referrer" });
    if (res.status === 404) throw new Error("this voice message was deleted");
    if (!res.ok || !res.body) throw new Error("this voice message couldn't be fetched");
    return readExactly(res.body, size);
  };
}
