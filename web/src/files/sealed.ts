/**
 * Files in encrypted chats, sealed on this device before they go anywhere.
 *
 * A file gets a fresh random AES-256-GCM key of its own. It's padded (one
 * 0x80 byte, then zeros) up to a size step, so the instance learns only
 * roughly how big it is, then sealed in chunks of `chunkBytes`: chunk i
 * under nonce = 4 zero bytes then i as 8 big-endian bytes, with the top bit
 * set on the last chunk. Reordering, cutting short or adding to the chunks
 * fails to open, and the key is never used for anything else, so the fixed
 * nonces are safe. The instance stores the sealed chunks one after another;
 * the key, the SHA-256 of those bytes, the file's name, type and size go
 * inside the encrypted message.
 *
 * Opened files are only shown inline when their own first bytes say they're
 * a picture or video type browsers draw safely (never what the sender said
 * they were); everything else is a download.
 */

import { Problem } from "../i18n/problem.ts";

/** How big the web app's chunks are. */
export const CHUNK_BYTES = 1024 * 1024;
/** The chunk sizes a device takes from a message. */
export const MIN_CHUNK = 64 * 1024;
export const MAX_CHUNK = 8 * 1024 * 1024;
/** The biggest file this app seals or opens: it's held in memory while it is. */
export const MAX_FILE_BYTES = 256 * 1024 * 1024;
/** Files a message can carry (the instance's limit too). */
export const MAX_FILES = 10;
const TAG = 16;

/**
 * The size a file is padded to, from its length plus the 0x80 marker: up to
 * 64 KiB the next 4 KiB, above that the next 1/16 of the power of two below,
 * so padding costs at most about 6%.
 */
export function paddedSize(length: number): number {
  const n = length + 1;
  if (n <= 64 * 1024) return Math.ceil(n / 4096) * 4096;
  const step = 2 ** Math.floor(Math.log2(n)) / 16;
  return Math.ceil(n / step) * step;
}

/** How many bytes the instance keeps for a file padded to `padded`. */
export function sealedSize(padded: number, chunkBytes: number): number {
  return padded + Math.ceil(padded / chunkBytes) * TAG;
}

/**
 * Whether `size` stored bytes can be a file sealed in chunks of
 * `chunkBytes`: checked before fetching anything a message names.
 */
export function plausible(size: number, chunkBytes: number): boolean {
  if (!Number.isSafeInteger(size) || chunkBytes < MIN_CHUNK || chunkBytes > MAX_CHUNK) return false;
  if (size <= TAG || size > sealedSize(paddedSize(MAX_FILE_BYTES), chunkBytes)) return false;
  const whole = chunkBytes + TAG;
  const last = size - Math.floor((size - 1) / whole) * whole;
  return last > TAG;
}

export type SealedFile = {
  /** The stored bytes, to upload. */
  bytes: Uint8Array<ArrayBuffer>;
  key: Uint8Array<ArrayBuffer>;
  sha256: Uint8Array<ArrayBuffer>;
  chunkBytes: number;
};

const subtle = () => {
  if (!globalThis.crypto?.subtle) throw new Problem("system.files.needsHttps");
  return crypto.subtle;
};

function nonce(index: number, last: boolean): Uint8Array<ArrayBuffer> {
  const n = new Uint8Array(12);
  const view = new DataView(n.buffer);
  view.setUint32(4, Math.floor(index / 2 ** 32) | (last ? 0x8000_0000 : 0));
  view.setUint32(8, index >>> 0);
  return n;
}

/** Seals a file on this device: padded, then sealed chunk by chunk under a key of its own. */
export async function sealFile(file: Blob, chunkBytes = CHUNK_BYTES): Promise<SealedFile> {
  if (file.size > MAX_FILE_BYTES) throw new Problem("system.files.tooBig");
  const padded = paddedSize(file.size);
  const count = Math.ceil(padded / chunkBytes);
  const key = crypto.getRandomValues(new Uint8Array(32));
  const k = await subtle().importKey("raw", key, "AES-GCM", false, ["encrypt"]);
  const bytes = new Uint8Array(sealedSize(padded, chunkBytes));
  for (let i = 0; i < count; i++) {
    const start = i * chunkBytes;
    const end = Math.min(padded, start + chunkBytes);
    const plain = new Uint8Array(end - start);
    if (start < file.size) plain.set(new Uint8Array(await file.slice(start, Math.min(end, file.size)).arrayBuffer()));
    if (file.size >= start && file.size < end) plain[file.size - start] = 0x80;
    const sealed = await subtle().encrypt({ name: "AES-GCM", iv: nonce(i, i === count - 1) }, k, plain);
    bytes.set(new Uint8Array(sealed), start + i * TAG);
  }
  const sha256 = new Uint8Array(await subtle().digest("SHA-256", bytes));
  return { bytes, key, sha256, chunkBytes };
}

const same = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((x, i) => x === b[i]);

/**
 * Opens a sealed file, after checking its bytes are the ones the message
 * named. Throws if anything's off: a changed, cut short, extended or
 * reordered file doesn't open.
 */
export async function openFile(bytes: Uint8Array<ArrayBuffer>, key: Uint8Array, sha256: Uint8Array, chunkBytes: number): Promise<Blob> {
  if (key.length !== 32 || !plausible(bytes.length, chunkBytes)) throw new Problem("system.files.cantOpen");
  const digest = new Uint8Array(await subtle().digest("SHA-256", bytes));
  if (!same(digest, sha256)) throw new Problem("system.files.notTheSame");
  const k = await subtle().importKey("raw", new Uint8Array(key), "AES-GCM", false, ["decrypt"]);
  const whole = chunkBytes + TAG;
  const count = Math.ceil(bytes.length / whole);
  const parts: Uint8Array<ArrayBuffer>[] = [];
  for (let i = 0; i < count; i++) {
    const chunk = bytes.subarray(i * whole, Math.min(bytes.length, (i + 1) * whole));
    parts.push(new Uint8Array(await subtle().decrypt({ name: "AES-GCM", iv: nonce(i, i === count - 1) }, k, chunk)));
  }
  // The marker is in the last chunk, or the one before when it ended a chunk exactly.
  let p = parts.length - 1;
  let end = parts[p]!.length - 1;
  for (;;) {
    while (end >= 0 && parts[p]![end] === 0) end--;
    if (end >= 0 || p === 0) break;
    parts.pop();
    p--;
    end = parts[p]!.length - 1;
  }
  if (end < 0 || parts[p]![end] !== 0x80) throw new Problem("system.files.cantOpen");
  parts[p] = parts[p]!.slice(0, end);
  return new Blob(parts, { type: "application/octet-stream" });
}

/** What an opened file can be shown as, from its own first bytes. */
export type Preview = { kind: "image" | "video"; type: string } | null;

const ascii = (b: Uint8Array, at: number, text: string) => [...text].every((c, i) => b[at + i] === c.charCodeAt(0));

/**
 * The picture or video type a file's first bytes say it is, of the few
 * shown inline (PNG, JPEG, GIF, WebP, AVIF, MP4, WebM), or null. What the
 * sender called it never counts: SVG, HTML, PDF and the rest are downloads.
 */
export function sniff(head: Uint8Array): Preview {
  const b = head;
  if (b.length >= 8 && [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a].every((x, i) => b[i] === x)) return { kind: "image", type: "image/png" };
  if (b.length >= 3 && b[0] === 0xff && b[1] === 0xd8 && b[2] === 0xff) return { kind: "image", type: "image/jpeg" };
  if (b.length >= 6 && (ascii(b, 0, "GIF87a") || ascii(b, 0, "GIF89a"))) return { kind: "image", type: "image/gif" };
  if (b.length >= 12 && ascii(b, 0, "RIFF") && ascii(b, 8, "WEBP")) return { kind: "image", type: "image/webp" };
  if (b.length >= 12 && ascii(b, 4, "ftyp")) {
    const brand = String.fromCharCode(...b.subarray(8, 12));
    if (brand === "avif" || brand === "avis") return { kind: "image", type: "image/avif" };
    if (["isom", "iso2", "iso4", "iso5", "iso6", "mp41", "mp42", "avc1", "dash", "M4V "].includes(brand)) return { kind: "video", type: "video/mp4" };
    return null;
  }
  if (b.length >= 4 && b[0] === 0x1a && b[1] === 0x45 && b[2] === 0xdf && b[3] === 0xa3) {
    // EBML: only the "webm" doc type (not other Matroska files).
    for (let i = 4; i + 4 <= Math.min(b.length, 64); i++) if (ascii(b, i, "webm")) return { kind: "video", type: "video/webm" };
  }
  return null;
}

/** Whether a picture's first bytes say it moves (an animated GIF or WebP), for reduce motion. */
export function animated(head: Uint8Array, type: string): boolean {
  // Any GIF may move: telling for sure means reading every block.
  if (type === "image/gif") return true;
  if (type === "image/webp") return head.length >= 21 && ascii(head, 12, "VP8X") && (head[20]! & 0x02) !== 0;
  return false;
}

/** Characters that turn text around or hide in it: never shown in a file's name. */
const TURNS = /[\u061c\u200b-\u200f\u2028-\u202e\u2060-\u2069\ufeff]/g;
// eslint-disable-next-line no-control-regex
const CONTROLS = /[\u0000-\u001f\u007f-\u009f]/g;

/**
 * A file's name as shown and saved: no path, no control characters or ones
 * that reverse text (so "gnp.exe" can't pass for a picture), at most 255
 * characters, never empty. The same rule the instance uses for attachments.
 */
export function cleanName(name: string): string {
  const last = name.split(/[/\\]/).pop() ?? "";
  const kept = [...last.replace(CONTROLS, "").replace(TURNS, "")].slice(0, 255).join("");
  const trimmed = kept.trim().replace(/^\.+/, "").trim();
  return trimmed || "file";
}
