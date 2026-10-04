import assert from "node:assert/strict";
import { test } from "node:test";
import { open, PAD_STEP, seal } from "./seal.ts";

test("sealed files open to what went in, padded to steps", async () => {
  for (const length of [0, 1, PAD_STEP - 1, PAD_STEP, 100_000]) {
    const plain = new Uint8Array(length).map((_, i) => (i * 31) & 0xff);
    const sealed = await seal(plain);
    assert.equal((sealed.bytes.length - 28) % PAD_STEP, 0);
    assert.ok(sealed.bytes.length - 28 > length);
    assert.deepEqual(await open(sealed.bytes, sealed.key, sealed.sha256), plain);
  }
});

test("a sealed file that's been changed doesn't open", async () => {
  const sealed = await seal(new Uint8Array([1, 2, 3]));
  const changed = sealed.bytes.slice();
  changed[40]! ^= 1;
  await assert.rejects(open(changed, sealed.key, sealed.sha256));
});
