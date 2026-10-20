import assert from "node:assert/strict";
import { test } from "node:test";
import { backdropFileKind, sanitizeBackdropFile } from "./device-backdrop.ts";

test("a stored device file is kept only when it names one this app kept", () => {
  const id = "0123456789abcdef01234567";
  assert.deepEqual(sanitizeBackdropFile({ id, kind: "video", name: "loop.mp4" }), { id, kind: "video", name: "loop.mp4" });
  assert.deepEqual(sanitizeBackdropFile({ id, kind: "image" }), { id, kind: "image", name: "" });
  assert.equal(sanitizeBackdropFile({ id: "../../etc", kind: "video", name: "x" }), null);
  assert.equal(sanitizeBackdropFile({ id, kind: "audio", name: "x" }), null);
  assert.equal(sanitizeBackdropFile(null), null);
  assert.equal(sanitizeBackdropFile("loop.mp4"), null);
  assert.equal(sanitizeBackdropFile({ id, kind: "image", name: "x".repeat(300) })!.name.length, 80);
});

test("videos and pictures are told apart by type, or by name when the type is missing", () => {
  assert.equal(backdropFileKind({ type: "video/mp4", name: "a.mp4" }), "video");
  assert.equal(backdropFileKind({ type: "video/webm", name: "a" }), "video");
  assert.equal(backdropFileKind({ type: "image/gif", name: "a.gif" }), "image");
  assert.equal(backdropFileKind({ type: "", name: "Loop.MOV" }), "video");
  assert.equal(backdropFileKind({ type: "", name: "cat.gif" }), "image");
  assert.equal(backdropFileKind({ type: "image/svg+xml", name: "a.svg" }), null);
  assert.equal(backdropFileKind({ type: "application/pdf", name: "a.mp4" }), null);
  assert.equal(backdropFileKind({ type: "", name: "notes.txt" }), null);
});
