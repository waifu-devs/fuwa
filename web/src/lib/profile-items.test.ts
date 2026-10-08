import assert from "node:assert/strict";
import { test } from "node:test";
import { BUILTIN_EFFECTS } from "./effects/profile.ts";
import {
  checkEffectText,
  ITEM_DECORATION,
  ITEM_EFFECT,
  itemEffect,
  itemsOfKind,
  nameFromFile,
  resolveDecoration,
  resolveEffect,
  wornDecoration,
  wornEffect,
  type Item,
} from "./profile-items.ts";

// Run with `pnpm test` (node's own test runner; no extra dependencies).

const layer = { shape: "star", motion: "twinkle", phase: "intro", count: 6, from: "edges", size: [8, 14], duration: [900, 1400], colors: ["#ffffff", "primary"] };
const spec = (extra: Record<string, unknown> = {}) => JSON.stringify({ id: "x", name: "x", layers: [layer, { ...layer, phase: "idle" }], ...extra });

const effect = (id: string, json = spec()): Item => ({ id, kind: ITEM_EFFECT, name: `Effect ${id}`, description: "Sparkly", effect: json, pictureUrl: "", animated: false });
const decoration = (id: string): Item => ({ id, kind: ITEM_DECORATION, name: `Decoration ${id}`, description: "", effect: "", pictureUrl: `https://fuwa.test/media/${id}`, animated: false });

const INSTANCE = [effect("01JINSTANCEEFFECT000000000"), decoration("01JINSTANCEDECOR0000000000")];
const SERVER = [effect("01JSERVEREFFECT00000000000"), decoration("01JSERVERDECOR000000000000")];

test("an effect item plays as a spec under its own id, name and description", () => {
  const played = itemEffect(INSTANCE[0]!);
  assert.ok(played);
  assert.equal(played.id, "01jinstanceeffect000000000");
  assert.equal(played.name, "Effect 01JINSTANCEEFFECT000000000");
  assert.equal(played.description, "Sparkly");
  assert.equal(played.layers.length, 2);
  // The same item gives the same spec, so an effect playing doesn't restart.
  assert.equal(itemEffect({ ...INSTANCE[0]! }), played);
});

test("specs are sanitized like any that didn't ship with the app", () => {
  const wild = itemEffect(effect("01JWILD0000000000000000000", spec({ layers: [{ ...layer, count: 5000, shape: "skull" }, { ...layer, count: 5000 }] })));
  assert.ok(wild);
  assert.equal(wild.layers.length, 1);
  assert.ok(wild.layers[0]!.count <= 24);
  assert.equal(itemEffect(effect("01JBROKEN00000000000000000", "{not json")), null);
  assert.equal(itemEffect(effect("01JEMPTY000000000000000000", spec({ layers: [] }))), null);
  assert.equal(itemEffect(decoration("01JDECOR000000000000000000")), null);
});

test("ids resolve to built-ins first, then the list given; unknown ids draw nothing", () => {
  const sakura = BUILTIN_EFFECTS[0]!;
  assert.equal(resolveEffect(sakura.id, INSTANCE), sakura);
  assert.equal(resolveEffect("01JINSTANCEEFFECT000000000", INSTANCE)?.id, "01jinstanceeffect000000000");
  assert.equal(resolveEffect("01jinstanceeffect000000000", INSTANCE)?.id, "01jinstanceeffect000000000");
  assert.equal(resolveEffect("01JINSTANCEDECOR0000000000", INSTANCE), undefined);
  assert.equal(resolveEffect("01JGONE0000000000000000000", INSTANCE), undefined);
  assert.equal(resolveEffect("", INSTANCE), undefined);
  assert.equal(resolveDecoration("01JINSTANCEDECOR0000000000", INSTANCE), INSTANCE[1]);
  assert.equal(resolveDecoration("01JINSTANCEEFFECT000000000", INSTANCE), undefined);
  assert.equal(resolveDecoration("01JINSTANCEDECOR0000000000", undefined), undefined);
});

test("a server profile's picks win in that server, from that server's list", () => {
  const lists = { instance: INSTANCE, server: SERVER };
  const user = { decorationId: "01JINSTANCEDECOR0000000000" };
  assert.equal(wornDecoration(user, undefined, lists), INSTANCE[1]);
  assert.equal(wornDecoration(user, { decorationId: "" }, lists), INSTANCE[1]);
  assert.equal(wornDecoration(user, { decorationId: "01JSERVERDECOR000000000000" }, lists), SERVER[1]);
  // A server pick it can't find draws nothing rather than the person's own.
  assert.equal(wornDecoration(user, { decorationId: "01JGONE0000000000000000000" }, lists), undefined);
  // A server profile can't wear the instance's items, nor the other way round.
  assert.equal(wornDecoration(user, { decorationId: "01JINSTANCEDECOR0000000000" }, lists), undefined);
  assert.equal(wornDecoration({ decorationId: "01JSERVERDECOR000000000000" }, undefined, lists), undefined);

  assert.equal(wornEffect("01JINSTANCEEFFECT000000000", undefined, lists)?.id, "01jinstanceeffect000000000");
  assert.equal(wornEffect("01JINSTANCEEFFECT000000000", { effect: "" }, lists)?.id, "01jinstanceeffect000000000");
  assert.equal(wornEffect("01JINSTANCEEFFECT000000000", { effect: "01JSERVEREFFECT00000000000" }, lists)?.id, "01jservereffect00000000000");
  assert.equal(wornEffect("01JINSTANCEEFFECT000000000", { effect: BUILTIN_EFFECTS[1]!.id }, lists), BUILTIN_EFFECTS[1]);
  assert.equal(wornEffect("", undefined, lists), undefined);
});

test("checks effects someone is about to add", () => {
  assert.deepEqual(checkEffectText("  "), { problem: "empty" });
  assert.deepEqual(checkEffectText("{nope"), { problem: "json" });
  assert.deepEqual(checkEffectText("[1, 2]"), { problem: "spec" });
  assert.deepEqual(checkEffectText(JSON.stringify({ layers: [{ shape: "skull" }] })), { problem: "spec" });
  const ok = checkEffectText(spec(), "Stars");
  assert.ok("spec" in ok);
  assert.equal(ok.spec.name, "Stars");
  // Its own id doesn't matter (the instance gives it the item's), so specs without one pass.
  assert.ok("spec" in checkEffectText(JSON.stringify({ layers: [layer] })));
});

test("lists by kind, and names from files", () => {
  assert.deepEqual(itemsOfKind(INSTANCE, ITEM_DECORATION), [INSTANCE[1]]);
  assert.deepEqual(itemsOfKind(undefined, ITEM_EFFECT), []);
  assert.equal(nameFromFile("cherry_blossom-frame.png"), "cherry blossom frame");
  assert.equal(nameFromFile("x".repeat(60) + ".gif").length, 40);
});
