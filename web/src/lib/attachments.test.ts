import assert from "node:assert/strict";
import { test } from "node:test";
import { cantAdd, extensionOf, familyOf, fitBox, localLook, lookOf, MAX_FILES, shortName } from "./attachments.ts";

// Run with `pnpm test` (node's own test runner; no extra dependencies).

test("files are shown by the kind the server found", () => {
  assert.equal(lookOf("image/png"), "picture");
  assert.equal(lookOf("video/mp4"), "video");
  assert.equal(lookOf("audio/ogg"), "audio");
  assert.equal(lookOf("application/zip"), "file");
  // Pictures the server doesn't serve as pictures stay files.
  assert.equal(lookOf("image/svg+xml"), "file");
  assert.equal(localLook({ type: "image/svg+xml", name: "logo.svg" }), "file");
  assert.equal(localLook({ type: "video/x-matroska", name: "clip.mkv" }), "file");
  assert.equal(localLook({ type: "video/webm", name: "clip.webm" }), "video");
});

test("names say what family a file is", () => {
  assert.equal(extensionOf("notes.tar.gz"), "gz");
  assert.equal(extensionOf(".bashrc"), "");
  assert.equal(extensionOf("README"), "");
  assert.equal(familyOf("build.ZIP"), "archive");
  assert.equal(familyOf("main.rs"), "code");
  assert.equal(familyOf("report.pdf"), "pdf");
  assert.equal(familyOf("mystery.bin"), "other");
});

test("long names keep their ends", () => {
  assert.equal(shortName("short.txt"), "short.txt");
  const long = shortName("a-really-long-file-name-that-goes-on-and-on-v2.zip", 30);
  assert.equal(Array.from(long).length, 30);
  assert.ok(long.endsWith("-v2.zip"), long);
  assert.ok(long.includes("…"));
});

test("pictures keep their shape in their box", () => {
  assert.deepEqual(fitBox(4000, 3000, { width: 400, height: 300 }), { width: 400, height: 300 });
  assert.deepEqual(fitBox(1000, 4000, { width: 400, height: 300 }), { width: 75, height: 300 });
  assert.deepEqual(fitBox(100, 50, { width: 400, height: 300 }), { width: 100, height: 50 });
  // Unknown sizes get a 16:9 box.
  assert.deepEqual(fitBox(0, 0, { width: 400, height: 300 }), { width: 400, height: 225 });
});

test("at most ten files go at once", () => {
  assert.equal(cantAdd(0, MAX_FILES), "");
  assert.match(cantAdd(9, 2), /up to 10/);
});
