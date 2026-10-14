import assert from "node:assert/strict";
import { test } from "node:test";
import { filesIn } from "./pasted.ts";

const png = (name: string) => new File([new Uint8Array([137, 80, 78, 71])], name, { type: "image/png" });
const item = (file: File | null, kind = "file") => ({ kind, type: file?.type ?? "text/plain", getAsFile: () => file }) as unknown as DataTransferItem;
const carrier = (files: File[], items: DataTransferItem[] = []) =>
  ({ files: files as unknown as FileList, items: items as unknown as DataTransferItemList });

test("every pasted or dropped file comes through, in order", () => {
  const [a, b] = [png("a.png"), png("b.png")];
  assert.deepEqual(filesIn(carrier([a, b])), [a, b]);
});

test("a picture only in the items (no files list) is still taken", () => {
  const shot = png("image.png");
  assert.deepEqual(filesIn(carrier([], [item(null, "string"), item(shot)])), [shot]);
});

test("text alone carries no files, so it goes in the box", () => {
  assert.deepEqual(filesIn(carrier([], [item(null, "string")])), []);
  assert.deepEqual(filesIn(null), []);
});
