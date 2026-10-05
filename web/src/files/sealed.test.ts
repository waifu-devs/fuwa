import assert from "node:assert/strict";
import { test } from "node:test";
import { cleanName, MIN_CHUNK, openFile, paddedSize, plausible, sealedSize, sealFile, sniff } from "./sealed.ts";

const bytesOf = async (blob: Blob) => new Uint8Array(await blob.arrayBuffer());

test("padding goes up in steps that cost at most about 6%", () => {
  assert.equal(paddedSize(0), 4096);
  assert.equal(paddedSize(4095), 4096);
  assert.equal(paddedSize(4096), 8192);
  assert.equal(paddedSize(65_535), 65_536);
  for (const n of [65_536, 100_000, 1_000_000, 123_456_789]) {
    const padded = paddedSize(n);
    assert.ok(padded > n && padded <= (n + 1) * 1.0625 + 1, `${n} -> ${padded}`);
  }
});

test("sealed files open to what went in, at every edge of a chunk", async () => {
  const chunk = MIN_CHUNK;
  for (const length of [0, 1, 4095, chunk - 1, chunk, chunk + 1, 3 * chunk - 1, 3 * chunk, 200_000]) {
    const plain = new Uint8Array(length).map((_, i) => (i * 31 + 7) & 0xff);
    const sealed = await sealFile(new Blob([plain]), chunk);
    assert.equal(sealed.bytes.length, sealedSize(paddedSize(length), chunk));
    assert.ok(plausible(sealed.bytes.length, chunk));
    assert.deepEqual(await bytesOf(await openFile(sealed.bytes, sealed.key, sealed.sha256, chunk)), plain);
  }
});

test("a changed, cut, extended or reordered file doesn't open", async () => {
  const chunk = MIN_CHUNK;
  const plain = new Uint8Array(3 * chunk).fill(9);
  const sealed = await sealFile(new Blob([plain]), chunk);
  const digest = async (b: Uint8Array<ArrayBuffer>) => new Uint8Array(await crypto.subtle.digest("SHA-256", b));
  const tries: Uint8Array<ArrayBuffer>[] = [];
  const changed = sealed.bytes.slice();
  changed[100]! ^= 1;
  tries.push(changed);
  // Cut to whole chunks: the new last one isn't marked last.
  tries.push(sealed.bytes.slice(0, 2 * (chunk + 16)));
  const whole = chunk + 16;
  const swapped = sealed.bytes.slice();
  swapped.set(sealed.bytes.subarray(whole, 2 * whole), 0);
  swapped.set(sealed.bytes.subarray(0, whole), whole);
  tries.push(swapped);
  const longer = new Uint8Array(sealed.bytes.length + whole);
  longer.set(sealed.bytes);
  longer.set(sealed.bytes.subarray(0, whole), sealed.bytes.length);
  tries.push(longer);
  for (const bytes of tries) {
    // Even with the right hash for what's there, it doesn't open.
    await assert.rejects(openFile(bytes, sealed.key, await digest(bytes), chunk));
  }
  await assert.rejects(openFile(sealed.bytes, sealed.key, sealed.sha256, chunk * 2));
});

test("chunk sizes and stored lengths are checked before fetching", () => {
  assert.ok(!plausible(100_000, 1024));
  assert.ok(!plausible(100_000, 64 * 1024 * 1024));
  // A last chunk with nothing but its tag can't be.
  assert.ok(!plausible(MIN_CHUNK + 16 + 16, MIN_CHUNK));
  assert.ok(plausible(MIN_CHUNK + 16 + 17, MIN_CHUNK));
  assert.ok(!plausible(16, MIN_CHUNK));
});

test("only a file's own bytes make it a picture or video", () => {
  const head = (...parts: (number[] | string)[]) =>
    new Uint8Array(parts.flatMap((p) => (typeof p === "string" ? [...p].map((c) => c.charCodeAt(0)) : p)));
  assert.equal(sniff(head([0x89], "PNG", [0x0d, 0x0a, 0x1a, 0x0a]))?.type, "image/png");
  assert.equal(sniff(head([0xff, 0xd8, 0xff, 0xe0]))?.type, "image/jpeg");
  assert.equal(sniff(head("GIF89a"))?.type, "image/gif");
  assert.equal(sniff(head("RIFF", [0, 0, 0, 0], "WEBP"))?.type, "image/webp");
  assert.equal(sniff(head([0, 0, 0, 0x1c], "ftypavif"))?.type, "image/avif");
  assert.equal(sniff(head([0, 0, 0, 0x1c], "ftypisom"))?.kind, "video");
  assert.equal(sniff(head([0x1a, 0x45, 0xdf, 0xa3, 0x9f, 0x42, 0x82, 0x84], "webm"))?.type, "video/webm");
  for (const other of ["<svg xmlns", "<!doctype html>", "%PDF-1.7", "<?xml version"]) assert.equal(sniff(head(other)), null);
  assert.equal(sniff(head([0, 0, 0, 0x14], "ftypqt  ")), null);
  assert.equal(sniff(head([0x1a, 0x45, 0xdf, 0xa3], "matroska")), null);
});

test("names lose paths and characters that hide or turn text", () => {
  assert.equal(cleanName("../../etc/passwd"), "passwd");
  assert.equal(cleanName("C:\\Users\\me\\cat.png"), "cat.png");
  assert.equal(cleanName("photo\u202egnp.exe"), "photognp.exe");
  assert.equal(cleanName("a\u0000b\nc"), "abc");
  assert.equal(cleanName("...hidden"), "hidden");
  assert.equal(cleanName("  \u200b "), "file");
  assert.equal(cleanName("x".repeat(400)).length, 255);
});
