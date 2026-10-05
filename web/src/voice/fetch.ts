/*
 * Fetching a voice message's file, wherever it's kept: never more than a
 * long voice message can be, nor more than the message says it is.
 */
import { i18n } from "@/i18n/i18n";
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
      if (at + value.length > size) throw new Error(i18n().t("system.voice.notTheOne"));
      out.set(value, at);
      at += value.length;
    }
  } finally {
    void reader.cancel().catch(() => {});
  }
  if (at !== size) throw new Error(i18n().t("system.voice.notTheOne"));
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
    if (!from) throw new Error(i18n().t("system.voice.notOnInstance"));
    if (!(size > 0) || size > MAX_VOICE_BYTES) throw new Error(i18n().t("system.voice.tooBig"));
    const res = await fetch(from, { credentials: "omit", referrerPolicy: "no-referrer" });
    if (res.status === 404) throw new Error(i18n().t("system.voice.deleted"));
    if (!res.ok || !res.body) throw new Error(i18n().t("system.voice.cantFetch"));
    return readExactly(res.body, size);
  };
}
