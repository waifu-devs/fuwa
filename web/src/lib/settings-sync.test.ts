import assert from "node:assert/strict";
import { test } from "node:test";
import type { Prefs } from "./prefs.ts";
import { firstStamps, FORMAT, localEntries, merge, readDocument, sameValue, stamp, writeDocument } from "./settings-sync.ts";

const prefs = (p: Partial<Prefs>) => ({ theme: "sakura", zoom: 100, font: { kind: "default" }, keybinds: {}, ...p }) as unknown as Prefs;

test("the document reads back what was written, and nothing from anything else", () => {
  const entries = { theme: { at: 5, value: "yoru" }, someday: { at: 7, value: [1, 2] } };
  assert.deepEqual(readDocument(writeDocument(entries)), entries);
  assert.deepEqual(readDocument(""), {});
  assert.deepEqual(readDocument("not json"), {});
  assert.deepEqual(readDocument(JSON.stringify({ format: "other", settings: entries })), {});
  assert.deepEqual(readDocument(JSON.stringify({ format: FORMAT, settings: { theme: { at: "soon", value: 1 }, clock: { at: 3 } } })), {});
});

test("only travelling settings changed here are stamped, past what they had", () => {
  const before = prefs({});
  const none = {};
  assert.equal(stamp(before, before, none, 100), none);
  assert.deepEqual(stamp(before, prefs({ zoom: 120 }), {}, 100), {});
  assert.deepEqual(stamp(before, prefs({ theme: "yoru" }), {}, 100), { theme: 100 });
  // A clock behind the stamp taken from another device still wins over it.
  assert.deepEqual(stamp(before, prefs({ theme: "yoru" }), { theme: 500 }, 100), { theme: 501 });
  // The same value in another key order isn't a change.
  assert.ok(sameValue({ a: 1, b: { c: 2, d: 3 } }, { b: { d: 3, c: 2 }, a: 1 }));
  assert.deepEqual(stamp(prefs({ keybinds: { a: "X", b: "Y" } }), prefs({ keybinds: { b: "Y", a: "X" } }), {}, 100), {});
});

test("a device from before sync offers what it changed, older than anything kept", () => {
  assert.deepEqual(firstStamps(prefs({ theme: "yoru", zoom: 130 }), prefs({})), { theme: 1 });
});

test("only stamped settings leave, and never a font from a file", () => {
  const p = prefs({ theme: "yoru", font: { kind: "file", id: "f", name: "Mine" }, codeFont: { kind: "installed", family: "Iosevka" } } as Partial<Prefs>);
  assert.deepEqual(localEntries(p, { theme: 3, font: 4, codeFont: 5 }), {
    theme: { at: 3, value: "yoru" },
    codeFont: { at: 5, value: { kind: "installed", family: "Iosevka" } },
  });
});

test("merging takes the later stamp, the instance's on a tie, and keeps what this app doesn't know", () => {
  const local = { theme: { at: 10, value: "yoru" }, clock: { at: 5, value: "24h" }, volume: { at: 7, value: 30 } };
  const remote = {
    theme: { at: 8, value: "sakura" },
    clock: { at: 6, value: "12h" },
    volume: { at: 7, value: 80 },
    overlayCorner: { at: 2, value: "top-left" },
    font: { at: 9, value: { kind: "file", id: "x", name: "x" } },
  };
  const { merged, incoming, behind } = merge(local, remote);
  assert.equal(behind, true);
  assert.deepEqual(merged.theme, local.theme);
  assert.deepEqual(merged.clock, remote.clock);
  assert.deepEqual(merged.volume, remote.volume);
  assert.deepEqual(merged.overlayCorner, remote.overlayCorner);
  // Settings this app doesn't know and fonts from files aren't taken.
  assert.deepEqual(incoming, { clock: remote.clock });
});

test("a new device takes everything and has nothing to send", () => {
  const remote = { theme: { at: 8, value: "yoru" } };
  assert.deepEqual(merge({}, remote), { merged: remote, incoming: remote, behind: false });
  assert.deepEqual(merge({ theme: { at: 1, value: "kawaii" } }, remote).behind, false);
  // And an instance with none gets this device's.
  assert.deepEqual(merge({ theme: { at: 1, value: "kawaii" } }, {}).behind, true);
});
