/**
 * The message backup's recovery key and its encryption. The key is 32
 * random bytes that never leave the person's devices; from it come the key
 * check the instance keeps (so a device can tell it has the right key) and
 * the AES-256-GCM key every backup part is sealed with, both by HKDF-SHA256.
 * A part's associated data names the account, so one person's parts can't
 * be passed off as another's.
 */

const SALT = new TextEncoder().encode("fuwa backup v1");
/** Crockford's base 32: no I, L, O or U, so it reads back without mix-ups. */
const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const KEY_BYTES = 32;
/** A checksum on the end, so a mistyped key is caught before anything is downloaded. */
const SUM_BYTES = 3;
const NONCE_BYTES = 12;
/** Parts are padded to a multiple of this, so their size says little about what's in them. */
export const PAD_TO = 4096;

/** A copy on a plain ArrayBuffer, as WebCrypto takes. */
const plain = (b: Uint8Array) => new Uint8Array(b) as Uint8Array<ArrayBuffer>;

export type BackupKeys = { check: Uint8Array; seal: CryptoKey };

export function newRecoveryKey(): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(KEY_BYTES));
}

async function checksum(key: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", plain(key))).slice(0, SUM_BYTES);
}

/** The key as people write it down: 56 characters in groups of four. */
export async function formatRecoveryKey(key: Uint8Array): Promise<string> {
  const bytes = new Uint8Array([...key, ...(await checksum(key))]);
  let bits = 0;
  let value = 0;
  let out = "";
  for (const byte of bytes) {
    value = ((value << 8) | byte) & 0xffff;
    bits += 8;
    while (bits >= 5) {
      out += ALPHABET[(value >>> (bits - 5)) & 31];
      bits -= 5;
    }
  }
  if (bits > 0) out += ALPHABET[(value << (5 - bits)) & 31];
  return out.match(/.{1,4}/g)!.join("-");
}

/** Reads a key back, forgiving spaces, dashes, case and the letters people confuse with digits. Null if it isn't one. */
export async function parseRecoveryKey(text: string): Promise<Uint8Array | null> {
  const clean = text
    .toUpperCase()
    .replace(/[\s-]/g, "")
    .replace(/O/g, "0")
    .replace(/[IL]/g, "1");
  let bits = 0;
  let value = 0;
  const bytes: number[] = [];
  for (const ch of clean) {
    const n = ALPHABET.indexOf(ch);
    if (n < 0) return null;
    value = ((value << 5) | n) & 0xffff;
    bits += 5;
    if (bits >= 8) {
      bytes.push((value >>> (bits - 8)) & 0xff);
      bits -= 8;
    }
  }
  if (bytes.length !== KEY_BYTES + SUM_BYTES) return null;
  const key = new Uint8Array(bytes.slice(0, KEY_BYTES));
  const sum = await checksum(key);
  return sum.every((b, i) => b === bytes[KEY_BYTES + i]) ? key : null;
}

export async function deriveKeys(key: Uint8Array): Promise<BackupKeys> {
  const base = await crypto.subtle.importKey("raw", plain(key), "HKDF", false, ["deriveBits", "deriveKey"]);
  const info = (s: string) => new TextEncoder().encode(s);
  const check = new Uint8Array(await crypto.subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt: SALT, info: info("check") }, base, 256));
  const seal = await crypto.subtle.deriveKey(
    { name: "HKDF", hash: "SHA-256", salt: SALT, info: info("encrypt") },
    base,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
  return { check, seal };
}

/** Binds a part to its account and its place in the backup, so it can't be moved to another of either. */
const associated = (accountId: string, sequence: bigint) => new TextEncoder().encode(`fuwa backup v1|${accountId}|${sequence}`);

/** A nonce, then the sealed bytes. */
export async function seal(keys: BackupKeys, accountId: string, sequence: bigint, plaintext: Uint8Array): Promise<Uint8Array> {
  const nonce = crypto.getRandomValues(new Uint8Array(NONCE_BYTES));
  const sealed = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce, additionalData: associated(accountId, sequence) }, keys.seal, plain(plaintext)),
  );
  const out = new Uint8Array(NONCE_BYTES + sealed.length);
  out.set(nonce);
  out.set(sealed, NONCE_BYTES);
  return out;
}

/** What a part said, or null if it wasn't sealed with these keys for this account and place. */
export async function open(keys: BackupKeys, accountId: string, sequence: bigint, data: Uint8Array): Promise<Uint8Array | null> {
  if (data.length <= NONCE_BYTES) return null;
  try {
    return new Uint8Array(
      await crypto.subtle.decrypt(
        { name: "AES-GCM", iv: plain(data.subarray(0, NONCE_BYTES)), additionalData: associated(accountId, sequence) },
        keys.seal,
        plain(data.subarray(NONCE_BYTES)),
      ),
    );
  } catch {
    return null;
  }
}

/** How many bytes a protobuf varint of `n` takes. */
const varintBytes = (n: number) => (n < 0x80 ? 1 : n < 0x4000 ? 2 : n < 0x200000 ? 3 : 4);

/**
 * How many zeros to put in a part's padding field (one tag byte, a varint
 * length, then the zeros) so a `bare`-byte part comes out at an exact multiple
 * of PAD_TO.
 */
export function paddingFor(bare: number): number {
  let total = Math.ceil((bare + 2) / PAD_TO) * PAD_TO;
  for (;;) {
    for (let len = 1; len <= 4; len++) {
      const zeros = total - bare - 1 - len;
      if (zeros >= 0 && varintBytes(zeros) === len) return zeros;
    }
    total += PAD_TO;
  }
}

export const sameBytes = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((x, i) => x === b[i]);

/** How a line compares with one already here. */
export type Version = { kind: string; deleted: boolean; editedAt: number };

/**
 * Whether a copy of a line from the backup should replace the one this
 * device has: a line it couldn't read, or one deleted or edited since. Parts
 * can come back in any order, so the newest version wins, not the last one.
 */
export function newer(have: Version | undefined, incoming: Version): boolean {
  if (!have || have.kind === "unreadable") return true;
  if (have.kind !== incoming.kind || have.deleted) return false;
  return incoming.deleted || incoming.editedAt > have.editedAt;
}
