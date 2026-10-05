import assert from "node:assert/strict";
import { test } from "node:test";
import { readOggOpus, writeOggOpus } from "./ogg.ts";
import { heights, waveform } from "./waveform.ts";

const packet = (n: number, size: number) => Uint8Array.from({ length: size }, (_, i) => (n * 31 + i) & 0xff);

test("an Ogg Opus file reads back as the packets written", () => {
  // Sizes around Ogg's 255-byte lacing edges, and enough packets for several pages.
  const sizes = [1, 254, 255, 256, 510, 600, ...Array.from({ length: 140 }, (_, i) => 40 + (i % 90))];
  const packets = sizes.map((size, n) => ({ data: packet(n, size), samples: 960 }));
  const file = writeOggOpus(packets, 1, 312);
  const read = readOggOpus(file);
  assert.equal(read.channels, 1);
  assert.equal(read.preSkip, 312);
  assert.equal(read.samples, sizes.length * 960);
  assert.equal(read.packets.length, packets.length);
  read.packets.forEach((p, n) => assert.deepEqual(p, packets[n]!.data));
  assert.equal(String.fromCharCode(...file.subarray(0, 4)), "OggS");
});

test("a damaged page is refused", () => {
  const file = writeOggOpus([{ data: packet(1, 80), samples: 960 }], 1, 312);
  file[file.length - 3] ^= 0xff;
  assert.throws(() => readOggOpus(file), /damaged/);
  assert.throws(() => readOggOpus(new TextEncoder().encode("<html>not ogg</html>")), /notOgg/);
});

test("waveforms fold levels into bytes and stretch back to any width", () => {
  const levels = Array.from({ length: 500 }, (_, i) => (i % 100) / 100);
  const bytes = waveform(levels);
  assert.equal(bytes.length, 64);
  assert.ok(Math.max(...bytes) === 255);
  assert.deepEqual(waveform([]), new Uint8Array(1));
  const bars = heights(bytes, 40);
  assert.equal(bars.length, 40);
  assert.ok(bars.every((h) => h >= 0 && h <= 1));
  assert.equal(heights(undefined, 10).length, 10);
});
