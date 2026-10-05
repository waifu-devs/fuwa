/**
 * Ogg Opus (RFC 7845), written and read by hand: voice messages are Opus
 * packets from WebCodecs put in an Ogg file here, so every browser makes
 * the same file and nothing else is needed. One logical stream, mono or
 * stereo, granule positions at 48 kHz.
 */

import { Problem } from "../i18n/problem.ts";

const CRC = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let r = i << 24;
    for (let j = 0; j < 8; j++) r = r & 0x80000000 ? (r << 1) ^ 0x04c11db7 : r << 1;
    table[i] = r >>> 0;
  }
  return table;
})();

function crc(bytes: Uint8Array): number {
  let c = 0;
  for (const b of bytes) c = ((c << 8) ^ CRC[((c >>> 24) ^ b) & 0xff]!) >>> 0;
  return c;
}

const ascii = (s: string) => Uint8Array.from(s, (ch) => ch.charCodeAt(0));

/** What starts an Ogg Opus file: its OpusHead packet. */
export function opusHead(channels: number, preSkip: number, inputRate = 48_000): Uint8Array {
  const head = new Uint8Array(19);
  const v = new DataView(head.buffer);
  head.set(ascii("OpusHead"));
  head[8] = 1;
  head[9] = channels;
  v.setUint16(10, preSkip, true);
  v.setUint32(12, inputRate, true);
  v.setInt16(16, 0, true);
  head[18] = 0;
  return head;
}

/** The OpusTags packet: who wrote it, and nothing about the person. */
export function opusTags(vendor = "fuwa"): Uint8Array {
  const name = ascii(vendor);
  const tags = new Uint8Array(8 + 4 + name.length + 4);
  const v = new DataView(tags.buffer);
  tags.set(ascii("OpusTags"));
  v.setUint32(8, name.length, true);
  tags.set(name, 12);
  v.setUint32(12 + name.length, 0, true);
  return tags;
}

/** One Ogg page holding whole packets. */
function page(packets: Uint8Array[], granule: bigint, serial: number, sequence: number, flags: number): Uint8Array {
  const lacing: number[] = [];
  for (const p of packets) {
    let left = p.length;
    while (left >= 255) {
      lacing.push(255);
      left -= 255;
    }
    lacing.push(left);
  }
  const body = packets.reduce((n, p) => n + p.length, 0);
  const out = new Uint8Array(27 + lacing.length + body);
  const v = new DataView(out.buffer);
  out.set(ascii("OggS"));
  out[4] = 0;
  out[5] = flags;
  v.setBigInt64(6, granule, true);
  v.setUint32(14, serial, true);
  v.setUint32(18, sequence, true);
  out[26] = lacing.length;
  out.set(lacing, 27);
  let at = 27 + lacing.length;
  for (const p of packets) {
    out.set(p, at);
    at += p.length;
  }
  v.setUint32(22, crc(out), true);
  return out;
}

const segments = (p: Uint8Array) => Math.floor(p.length / 255) + 1;

/**
 * An Ogg Opus file from Opus packets and how many 48 kHz samples each one
 * holds. `preSkip` is what the decoder drops from the start.
 */
export function writeOggOpus(packets: { data: Uint8Array; samples: number }[], channels: number, preSkip: number): Uint8Array {
  const serial = crypto.getRandomValues(new Uint32Array(1))[0]!;
  const pages: Uint8Array[] = [page([opusHead(channels, preSkip)], 0n, serial, 0, 0x02), page([opusTags()], 0n, serial, 1, 0)];
  let sequence = 2;
  let granule = BigInt(preSkip);
  let batch: Uint8Array[] = [];
  let count = 0;
  const flush = (last: boolean) => {
    if (!batch.length && !last) return;
    pages.push(page(batch, granule, serial, sequence++, last ? 0x04 : 0));
    batch = [];
    count = 0;
  };
  packets.forEach((p, i) => {
    // About a second of sound a page, and never more than a page can name.
    if (count + segments(p.data) > 255 || batch.length >= 50) flush(false);
    batch.push(p.data);
    count += segments(p.data);
    granule += BigInt(p.samples);
    if (i === packets.length - 1) flush(true);
  });
  if (!packets.length) flush(true);
  const size = pages.reduce((n, p) => n + p.length, 0);
  const file = new Uint8Array(size);
  let at = 0;
  for (const p of pages) {
    file.set(p, at);
    at += p.length;
  }
  return file;
}

/** What an Ogg Opus file holds: its channels, pre-skip, length and packets. */
export type OggOpus = { channels: number; preSkip: number; samples: number; packets: Uint8Array[] };

/**
 * Reads an Ogg Opus file's packets (one stream), checking each page's CRC.
 * Throws on anything that isn't one.
 */
export function readOggOpus(file: Uint8Array): OggOpus {
  const v = new DataView(file.buffer, file.byteOffset, file.byteLength);
  const packets: Uint8Array[] = [];
  let partial: Uint8Array[] = [];
  let at = 0;
  let lastGranule = 0n;
  while (at < file.length) {
    if (at + 27 > file.length || v.getUint32(at, false) !== 0x4f676753) throw new Problem("system.ogg.notOgg");
    const count = file[at + 26]!;
    const lacing = file.subarray(at + 27, at + 27 + count);
    const bodyLength = lacing.reduce((n, l) => n + l, 0);
    const end = at + 27 + count + bodyLength;
    if (end > file.length) throw new Problem("system.ogg.cutOff");
    const copy = file.slice(at, end);
    new DataView(copy.buffer).setUint32(22, 0, true);
    if (crc(copy) !== v.getUint32(at + 22, true)) throw new Problem("system.ogg.damaged");
    const granule = v.getBigInt64(at + 6, true);
    if (granule > 0n) lastGranule = granule;
    let body = at + 27 + count;
    let start = body;
    for (const l of lacing) {
      body += l;
      if (l < 255) {
        partial.push(file.subarray(start, body));
        const whole = partial.length === 1 ? partial[0]! : concat(partial);
        packets.push(whole);
        partial = [];
        start = body;
      }
    }
    if (start < body) partial.push(file.subarray(start, body));
    at = end;
  }
  const head = packets[0];
  if (!head || head.length < 19 || String.fromCharCode(...head.subarray(0, 8)) !== "OpusHead") throw new Problem("system.ogg.notOpus");
  const hv = new DataView(head.buffer, head.byteOffset, head.byteLength);
  const preSkip = hv.getUint16(10, true);
  return { channels: head[9]!, preSkip, samples: Math.max(0, Number(lastGranule) - preSkip), packets: packets.slice(2) };
}

function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}
