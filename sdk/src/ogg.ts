// Ogg Opus files (RFC 7845) in plain TypeScript: reading one into Opus
// packets an agent can say, and writing packets it heard into a file any
// player opens. No codec: the packets stay as they are.

/** How long an Opus packet plays, in milliseconds, from its first byte (RFC 6716, 3.1). */
export function opusPacketDuration(packet: Uint8Array): number {
  if (packet.length === 0) return 0;
  const toc = packet[0]!;
  const config = toc >> 3;
  const frameMs =
    config < 12 ? [10, 20, 40, 60][config % 4]! : config < 16 ? [10, 20][config % 2]! : [2.5, 5, 10, 20][config % 4]!;
  const code = toc & 3;
  const frames = code === 0 ? 1 : code === 3 ? (packet[1] ?? 0) & 0x3f : 2;
  return frameMs * frames;
}

export interface OpusHead {
  channels: number;
  /** Samples at 48 kHz to drop from the start. */
  preSkip: number;
  /** The rate the sound was recorded at, for information only: Opus always plays at 48 kHz. */
  inputSampleRate: number;
}

/**
 * The Opus packets in an Ogg Opus file, in order, leaving out its two
 * header packets. Reads the first Opus stream in the file.
 */
export function readOggOpus(file: Uint8Array): { head: OpusHead; packets: Uint8Array[] } {
  const view = new DataView(file.buffer, file.byteOffset, file.byteLength);
  let serial: number | undefined;
  let head: OpusHead | undefined;
  let tags = false;
  const packets: Uint8Array[] = [];
  let partial: Uint8Array[] = [];
  let pos = 0;
  while (pos + 27 <= file.length) {
    if (file[pos] !== 0x4f || file[pos + 1] !== 0x67 || file[pos + 2] !== 0x67 || file[pos + 3] !== 0x53) {
      throw new TypeError("not an Ogg file (or a damaged one): no OggS page here");
    }
    const pageSerial = view.getUint32(pos + 14, true);
    const segments = file[pos + 26]!;
    const table = pos + 27;
    let body = table + segments;
    if (body > file.length) throw new TypeError("the Ogg file ends in the middle of a page");
    const mine = serial === undefined || pageSerial === serial;
    for (let i = 0; i < segments; i++) {
      const len = file[table + i]!;
      if (body + len > file.length) throw new TypeError("the Ogg file ends in the middle of a page");
      if (mine) partial.push(file.subarray(body, body + len));
      body += len;
      if (len < 255 && mine) {
        const packet = join(partial);
        partial = [];
        if (!head) {
          if (!startsWith(packet, "OpusHead")) continue; // another stream's header
          serial = pageSerial;
          head = {
            channels: packet[9] ?? 1,
            preSkip: packet.length >= 12 ? packet[10]! | (packet[11]! << 8) : 0,
            inputSampleRate: packet.length >= 16 ? new DataView(packet.buffer, packet.byteOffset).getUint32(12, true) : 0,
          };
        } else if (!tags) {
          tags = true; // OpusTags
        } else if (packet.length > 0) {
          packets.push(packet);
        }
      }
    }
    pos = body;
  }
  if (!head) throw new TypeError("not an Ogg Opus file: no OpusHead");
  return { head, packets };
}

/**
 * Writes Opus packets into an Ogg Opus file: `add` each packet (20 ms each,
 * as voice channels carry them), then `finish` for the file's bytes.
 */
export class OggOpusWriter {
  readonly #serial: number;
  readonly #pages: Uint8Array[] = [];
  #sequence = 0;
  #granule = 0n;
  #pending: Uint8Array[] = [];
  #pendingSegments = 0;
  #done = false;

  constructor(options: { channels?: number; serial?: number } = {}) {
    this.#serial = options.serial ?? Math.floor(Math.random() * 0xffffffff);
    const head = new Uint8Array(19);
    head.set(ascii("OpusHead"));
    head[8] = 1; // version
    head[9] = options.channels ?? 1;
    // pre-skip 0, input rate 48000, gain 0, mapping 0
    new DataView(head.buffer).setUint32(12, 48_000, true);
    this.#page([head], 0n, 0x02);
    const vendor = ascii("fuwa sdk");
    const tags = new Uint8Array(8 + 4 + vendor.length + 4);
    tags.set(ascii("OpusTags"));
    new DataView(tags.buffer).setUint32(8, vendor.length, true);
    tags.set(vendor, 12);
    this.#page([tags], 0n, 0);
  }

  /** Adds one Opus packet. */
  add(packet: Uint8Array): void {
    if (this.#done) throw new Error("the file is already finished");
    const segments = Math.floor(packet.length / 255) + 1;
    if (this.#pendingSegments + segments > 255) this.#flush(0);
    this.#pending.push(packet);
    this.#pendingSegments += segments;
    this.#granule += BigInt(Math.round((opusPacketDuration(packet) || 20) * 48));
    // About a second of sound per page.
    if (this.#pending.length >= 50) this.#flush(0);
  }

  /** The whole file. */
  finish(): Uint8Array {
    if (!this.#done) {
      this.#flush(0x04, true);
      this.#done = true;
    }
    return join(this.#pages);
  }

  #flush(flags: number, force = false): void {
    if (this.#pending.length === 0 && !force) return;
    this.#page(this.#pending, this.#granule, flags);
    this.#pending = [];
    this.#pendingSegments = 0;
  }

  #page(packets: Uint8Array[], granule: bigint, flags: number): void {
    const lacing: number[] = [];
    for (const p of packets) {
      for (let n = p.length; ; n -= 255) {
        lacing.push(Math.min(n, 255));
        if (n < 255) break;
      }
    }
    const body = join(packets);
    const page = new Uint8Array(27 + lacing.length + body.length);
    const view = new DataView(page.buffer);
    page.set(ascii("OggS"));
    page[5] = flags;
    view.setBigUint64(6, granule, true);
    view.setUint32(14, this.#serial, true);
    view.setUint32(18, this.#sequence++, true);
    page[26] = lacing.length;
    page.set(lacing, 27);
    page.set(body, 27 + lacing.length);
    view.setUint32(22, crc32(page), true);
    this.#pages.push(page);
  }
}

function ascii(s: string): Uint8Array {
  return Uint8Array.from(s, (c) => c.charCodeAt(0));
}

function startsWith(bytes: Uint8Array, prefix: string): boolean {
  if (bytes.length < prefix.length) return false;
  for (let i = 0; i < prefix.length; i++) if (bytes[i] !== prefix.charCodeAt(i)) return false;
  return true;
}

function join(parts: Uint8Array[]): Uint8Array {
  if (parts.length === 1) return parts[0]!.slice();
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

// Ogg's CRC-32: polynomial 0x04c11db7, not reflected, starting at 0.
let table: Uint32Array | undefined;
function crc32(bytes: Uint8Array): number {
  if (!table) {
    table = new Uint32Array(256);
    for (let i = 0; i < 256; i++) {
      let r = i << 24;
      for (let j = 0; j < 8; j++) r = r & 0x80000000 ? (r << 1) ^ 0x04c11db7 : r << 1;
      table[i] = r >>> 0;
    }
  }
  let crc = 0;
  for (const b of bytes) crc = ((crc << 8) ^ table[((crc >>> 24) ^ b) & 0xff]!) >>> 0;
  return crc;
}
