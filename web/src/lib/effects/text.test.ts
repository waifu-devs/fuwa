import assert from "node:assert/strict";
import { test } from "node:test";
import { BUILTIN_EFFECTS } from "./profile.ts";
import { effectKeys } from "./text.ts";

test("every built-in effect has catalog keys", () => {
  for (const effect of BUILTIN_EFFECTS) assert.ok(effectKeys(effect.id), effect.id);
});

test("ids that name inherited properties get no keys", () => {
  for (const id of ["constructor", "toString", "__proto__", "hasOwnProperty"]) assert.equal(effectKeys(id), undefined, id);
});
