/**
 * Sealing a file for an encrypted message: AES-256-GCM under a fresh random
 * key, kept as the 12-byte nonce then the ciphertext and tag. The key and
 * the SHA-256 of the sealed bytes go inside the message; the sealed bytes
 * go to the instance, which can't open them.
 *
 * Before sealing, the file is padded (one 0x80 byte, then zeros) up to a
 * multiple of PAD_STEP, so its stored size tells the instance only roughly
 * how long a voice message is: within about eight seconds at 32 kbps.
 */

import { Problem } from "../i18n/problem.ts";

/** Sealed files' plaintext comes in steps of this many bytes. */
export const PAD_STEP = 32 * 1024;

function pad(plain: Uint8Array): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(Math.ceil((plain.length + 1) / PAD_STEP) * PAD_STEP);
  out.set(plain);
  out[plain.length] = 0x80;
  return out;
}

function unpad(padded: Uint8Array<ArrayBuffer>): Uint8Array<ArrayBuffer> {
  let end = padded.length - 1;
  while (end >= 0 && padded[end] === 0) end--;
  if (end < 0 || padded[end] !== 0x80) throw new Problem("system.files.cantOpen");
  return padded.slice(0, end);
}

export type Sealed = { bytes: Uint8Array<ArrayBuffer>; key: Uint8Array<ArrayBuffer>; sha256: Uint8Array<ArrayBuffer> };

const subtle = () => {
  if (!globalThis.crypto?.subtle) throw new Problem("system.files.needsHttps");
  return crypto.subtle;
};

export async function seal(plain: Uint8Array<ArrayBuffer>): Promise<Sealed> {
  const key = crypto.getRandomValues(new Uint8Array(32));
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const k = await subtle().importKey("raw", key, "AES-GCM", false, ["encrypt"]);
  const sealed = new Uint8Array(await subtle().encrypt({ name: "AES-GCM", iv: nonce }, k, pad(plain)));
  const bytes = new Uint8Array(12 + sealed.length);
  bytes.set(nonce);
  bytes.set(sealed, 12);
  const sha256 = new Uint8Array(await subtle().digest("SHA-256", bytes));
  return { bytes, key, sha256 };
}

const same = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((x, i) => x === b[i]);

/** Opens sealed bytes, after checking they're the ones the message named. Throws if anything's off. */
export async function open(bytes: Uint8Array<ArrayBuffer>, key: Uint8Array, sha256: Uint8Array): Promise<Uint8Array<ArrayBuffer>> {
  if (key.length !== 32 || bytes.length < 28) throw new Problem("system.files.cantOpen");
  const digest = new Uint8Array(await subtle().digest("SHA-256", bytes));
  if (!same(digest, sha256)) throw new Problem("system.files.notTheSame");
  const k = await subtle().importKey("raw", new Uint8Array(key), "AES-GCM", false, ["decrypt"]);
  return unpad(new Uint8Array(await subtle().decrypt({ name: "AES-GCM", iv: bytes.subarray(0, 12) }, k, bytes.subarray(12))));
}
