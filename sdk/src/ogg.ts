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
 * The 20 ms frames in an Opus packet, each as a packet of its own (RFC 6716,
 * 3.2): a packet holding two or three 20 ms frames, as some encoders make,
 * becomes two or three packets a voice channel takes, without re-encoding.
 * Throws a TypeError for a damaged packet.
 */
export function splitOpusPacket(packet: Uint8Array): Uint8Array[] {
  if (packet.length === 0) return [];
  const toc = packet[0]!;
  const code = toc & 3;
  const one = (frame: Uint8Array) => {
    const out = new Uint8Array(frame.length + 1);
    out[0] = toc & 0xfc;
    out.set(frame, 1);
    return out;
  };
  const damaged = () => new TypeError("a damaged Opus packet");
  const size = (at: number): [number, number] => {
    const b0 = packet[at];
    if (b0 === undefined) throw damaged();
    if (b0 < 252) return [b0, 1];
    const b1 = packet[at + 1];
    if (b1 === undefined) throw damaged();
    return [b0 + 4 * b1, 2];
  };
  if (code === 0) return [packet];
  if (code === 1) {
    if ((packet.length - 1) % 2) throw damaged();
    const half = (packet.length - 1) / 2;
    return [one(packet.subarray(1, 1 + half)), one(packet.subarray(1 + half))];
  }
  if (code === 2) {
    const [first, used] = size(1);
    const at = 1 + used;
    if (at + first > packet.length) throw damaged();
    return [one(packet.subarray(at, at + first)), one(packet.subarray(at + first))];
  }
  const header = packet[1];
  if (header === undefined) throw damaged();
  const count = header & 0x3f;
  if (count === 0) throw damaged();
  let at = 2;
  let end = packet.length;
  if (header & 0x40) {
    // Padding: bytes of 255 mean 254 and more to come.
    for (;;) {
      const b = packet[at++];
      if (b === undefined) throw damaged();
      end -= b === 255 ? 254 : b;
      if (b !== 255) break;
    }
  }
  const sizes: number[] = [];
  if (header & 0x80) {
    for (let i = 0; i < count - 1; i++) {
      const [n, used] = size(at);
      sizes.push(n);
      at += used;
    }
    sizes.push(end - at - sizes.reduce((a, b) => a + b, 0));
  } else {
    if ((end - at) % count) throw damaged();
    for (let i = 0; i < count; i++) sizes.push((end - at) / count);
  }
  if (end < at || sizes.some((n) => n < 0)) throw damaged();
  const frames: Uint8Array[] = [];
  for (const n of sizes) {
    if (at + n > end) throw damaged();
    frames.push(one(packet.subarray(at, at + n)));
    at += n;
  }
  return frames;
}

/**
 * The Opus packets in an Ogg Opus file, in order, leaving out its two
 * header packets. Reads the first Opus stream in the file.
 */
export function readOggOpus(file: Uint8Array): { head: OpusHead; packets: Uint8Array[] } {
  const reader = new OggOpusReader();
  const packets = reader.push(file);
  reader.end();
  return { head: reader.head!, packets };
}

const MAX_PACKET = 128 * 1024;
const MAX_SKIPPED = 1024 * 1024;

/**
 * Reads Ogg Opus as it arrives, from a download or a speech service, in
 * pieces of any size: `push` each piece for the packets it completes, and
 * `end` once there's no more.
 */
export class OggOpusReader {
  #buffer: Uint8Array = new Uint8Array(0);
  #serial: number | undefined;
  #head: OpusHead | undefined;
  #tags = false;
  #partial: Uint8Array[] = [];
  #partialBytes = 0;
  #skipped = 0;

  /** The stream's header, once it has come. */
  get head(): OpusHead | undefined {
    return this.#head;
  }

  /** Adds the next piece; returns the Opus packets it completed. */
  push(chunk: Uint8Array): Uint8Array[] {
    const file = this.#buffer.length ? join([this.#buffer, chunk]) : chunk;
    const view = new DataView(file.buffer, file.byteOffset, file.byteLength);
    const packets: Uint8Array[] = [];
    let pos = 0;
    while (pos + 27 <= file.length) {
      if (file[pos] !== 0x4f || file[pos + 1] !== 0x67 || file[pos + 2] !== 0x67 || file[pos + 3] !== 0x53) {
        throw new TypeError("not an Ogg file (or a damaged one): no OggS page here");
      }
      const segments = file[pos + 26]!;
      const table = pos + 27;
      if (table + segments > file.length) break;
      let size = 0;
      for (let i = 0; i < segments; i++) size += file[table + i]!;
      if (table + segments + size > file.length) break; // the rest of the page hasn't come yet
      const pageSerial = view.getUint32(pos + 14, true);
      const mine = this.#serial === undefined || pageSerial === this.#serial;
      let body = table + segments;
      for (let i = 0; i < segments; i++) {
        const len = file[table + i]!;
        if (mine) {
          this.#partial.push(file.slice(body, body + len));
          this.#partialBytes += len;
          // An Opus packet is at most about 61 KB: anything bigger is damaged or hostile.
          if (this.#partialBytes > MAX_PACKET) throw new TypeError("not an Ogg Opus file: a packet is far too big");
        }
        body += len;
        if (len < 255 && mine) {
          const packet = join(this.#partial);
          this.#partial = [];
          this.#partialBytes = 0;
          if (!this.#head) {
            if (!startsWith(packet, "OpusHead")) {
              // Another stream's header; there's only so much to look through.
              this.#skipped += packet.length;
              if (this.#skipped > MAX_SKIPPED) throw new TypeError("not an Ogg Opus file: no OpusHead");
              continue;
            }
            this.#serial = pageSerial;
            this.#head = {
              channels: packet[9] ?? 1,
              preSkip: packet.length >= 12 ? packet[10]! | (packet[11]! << 8) : 0,
              inputSampleRate:
                packet.length >= 16 ? new DataView(packet.buffer, packet.byteOffset).getUint32(12, true) : 0,
            };
          } else if (!this.#tags) {
            this.#tags = true; // OpusTags
          } else if (packet.length > 0) {
            packets.push(packet);
          }
        }
      }
      pos = body;
    }
    this.#buffer = file.slice(pos);
    return packets;
  }

  /** Says there's no more; throws if the stream stopped part way. */
  end(): void {
    if (this.#buffer.length) throw new TypeError("the Ogg file ends in the middle of a page");
    if (!this.#head) throw new TypeError("not an Ogg Opus file: no OpusHead");
  }
}

/**
 * The Opus packets of Ogg Opus arriving in pieces (a fetch body, a file
 * stream), as they complete.
 */
export async function* oggOpusPackets(
  source: AsyncIterable<Uint8Array> | Iterable<Uint8Array> | ReadableStream<Uint8Array>,
): AsyncGenerator<Uint8Array> {
  const reader = new OggOpusReader();
  for await (const chunk of chunks(source)) yield* reader.push(chunk);
  reader.end();
}

/** Pieces from a stream, async iterable or not (older browsers' ReadableStream isn't). */
export async function* chunks(
  source: AsyncIterable<Uint8Array> | Iterable<Uint8Array> | ReadableStream<Uint8Array>,
): AsyncGenerator<Uint8Array> {
  if (Symbol.asyncIterator in source || Symbol.iterator in source) {
    yield* source as AsyncIterable<Uint8Array>;
    return;
  }
  const reader = (source as ReadableStream<Uint8Array>).getReader();
  let done = false;
  try {
    for (;;) {
      const next = await reader.read();
      if (next.done) {
        done = true;
        return;
      }
      yield next.value;
    }
  } finally {
    // Stopped part way (talked over, say): let the download go.
    if (!done) await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

/**
 * Writes Opus packets into an Ogg Opus file: `add` each packet (20 ms each,
 * as voice channels carry them), then `finish` for the file's bytes. To send
 * the file while it's being written (to a speech service, say), `flush` and
 * `take` the pages so far; `finish` then gives the rest.
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

  /** Ends the page being written, so `take` has every packet added so far. */
  flush(): void {
    if (!this.#done) this.#flush(0);
  }

  /** The bytes written since the last `take` (the whole file so far, the first time). */
  take(): Uint8Array<ArrayBuffer> {
    const out = join(this.#pages);
    this.#pages.length = 0;
    return out;
  }

  /** Ends the file: all of it, or what's left after `take`. */
  finish(): Uint8Array<ArrayBuffer> {
    if (!this.#done) {
      this.#flush(0x04, true);
      this.#done = true;
    }
    return this.take();
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

function join(parts: Uint8Array[]): Uint8Array<ArrayBuffer> {
  if (parts.length === 0) return new Uint8Array(0);
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
