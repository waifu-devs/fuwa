import assert from "node:assert/strict";
import { test } from "node:test";
import { deriveKeys, formatRecoveryKey, newer, newRecoveryKey, open, PAD_TO, paddingFor, parseRecoveryKey, seal } from "./backupkey.ts";

test("a recovery key reads back as written, forgiving case, spaces and look-alike letters", async () => {
  for (let n = 0; n < 20; n++) {
    const key = newRecoveryKey();
    const text = await formatRecoveryKey(key);
    assert.match(text, /^([0-9A-HJKMNP-TV-Z]{4}-){13}[0-9A-HJKMNP-TV-Z]{4}$/);
    assert.deepEqual(await parseRecoveryKey(text), key);
    const sloppy = text.toLowerCase().replaceAll("-", " ").replaceAll("0", "o").replaceAll("1", "l");
    assert.deepEqual(await parseRecoveryKey(sloppy), key);
  }
});

test("a mistyped or short recovery key is caught", async () => {
  const text = await formatRecoveryKey(newRecoveryKey());
  const i = 7;
  const typo = text.slice(0, i) + (text[i] === "A" ? "B" : "A") + text.slice(i + 1);
  assert.equal(await parseRecoveryKey(typo), null);
  assert.equal(await parseRecoveryKey(text.slice(0, -5)), null);
  assert.equal(await parseRecoveryKey("not a key at all!"), null);
});

test("parts open only with the same key, for the same account and place", async () => {
  const a = await deriveKeys(newRecoveryKey());
  const b = await deriveKeys(newRecoveryKey());
  assert.equal(a.check.length, 32);
  assert.notDeepEqual(a.check, b.check);
  const secret = new TextEncoder().encode("Friday at noon");
  const part = await seal(a, "juan", 3n, secret);
  assert.deepEqual(await open(a, "juan", 3n, part), secret);
  assert.equal(await open(b, "juan", 3n, part), null, "another key");
  assert.equal(await open(a, "mika", 3n, part), null, "another account");
  assert.equal(await open(a, "juan", 4n, part), null, "another place");
  part[part.length - 1] ^= 1;
  assert.equal(await open(a, "juan", 3n, part), null, "changed");
});

test("the same key always gives the same check", async () => {
  const key = newRecoveryKey();
  assert.deepEqual((await deriveKeys(key)).check, (await deriveKeys(key)).check);
});

test("the newest version of a line wins, whatever order parts come in", () => {
  const text = (editedAt = 0, deleted = false) => ({ kind: "text", editedAt, deleted });
  assert.equal(newer(undefined, text()), true);
  assert.equal(newer({ kind: "unreadable", editedAt: 0, deleted: false }, text()), true);
  assert.equal(newer(text(), text()), false);
  assert.equal(newer(text(5), text(9)), true);
  assert.equal(newer(text(9), text(5)), false);
  assert.equal(newer(text(9), text(0, true)), true, "deleted beats edited");
  assert.equal(newer(text(0, true), text(9)), false, "nothing brings a deleted line back");
  assert.equal(newer({ kind: "devices", editedAt: 0, deleted: false }, text()), false);
});

test("padding brings every part to an exact multiple of 4 KiB", () => {
  const varint = (n: number) => (n < 0x80 ? 1 : n < 0x4000 ? 2 : n < 0x200000 ? 3 : 4);
  for (let bare = 0; bare < 3 * PAD_TO + 50; bare++) {
    const zeros = paddingFor(bare);
    const total = bare + 1 + varint(zeros) + zeros;
    assert.equal(total % PAD_TO, 0, `bare ${bare}`);
    // A size right where the length grows a byte can't land on the step, so it goes to the next one.
    assert.ok(total - bare <= 2 * PAD_TO, `bare ${bare} padded too much`);
  }
  for (const bare of [200 * 1024, 200 * 1024 + 4093]) assert.equal((bare + 1 + varint(paddingFor(bare)) + paddingFor(bare)) % PAD_TO, 0);
});
