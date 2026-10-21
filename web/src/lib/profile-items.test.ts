import assert from "node:assert/strict";
import { test } from "node:test";
import { BUILTIN_EFFECTS, builtinEffect } from "./effects/profile.ts";
import {
  checkEffectText,
  DECORATION_PROMPT,
  EFFECT_PROMPT,
  ITEM_DECORATION,
  ITEM_EFFECT,
  itemEffect,
  itemsOfKind,
  joinShader,
  nameFromFile,
  resolveDecoration,
  resolveEffect,
  splitShader,
  starterText,
  TEMPLATE_AVATAR,
  TEMPLATE_SIZE,
  wornDecoration,
  wornEffect,
  wornNameplate,
  ITEM_NAMEPLATE,
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

test("nameplates are worn like decorations, and only as nameplates", () => {
  const plate = (id: string): Item => ({ ...decoration(id), kind: ITEM_NAMEPLATE, name: `Nameplate ${id}` });
  const lists = { instance: [...INSTANCE, plate("01JINSTANCEPLATE0000000000")], server: [...SERVER, plate("01JSERVERPLATE000000000000")] };
  const user = { nameplateId: "01JINSTANCEPLATE0000000000" };
  assert.equal(wornNameplate(user, undefined, lists)?.id, "01JINSTANCEPLATE0000000000");
  assert.equal(wornNameplate(user, { nameplateId: "" }, lists)?.id, "01JINSTANCEPLATE0000000000");
  assert.equal(wornNameplate(user, { nameplateId: "01JSERVERPLATE000000000000" }, lists)?.id, "01JSERVERPLATE000000000000");
  // A decoration's id isn't a nameplate.
  assert.equal(wornNameplate({ nameplateId: "01JINSTANCEDECOR0000000000" }, undefined, lists), undefined);
  assert.equal(wornDecoration({ decorationId: "01JINSTANCEPLATE0000000000" }, undefined, lists), undefined);
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

test("every starter is its built-in's layers, laid out to read", () => {
  for (const effect of BUILTIN_EFFECTS) {
    const text = starterText(effect.id)!;
    assert.deepEqual(JSON.parse(text), { layers: effect.layers }, effect.id);
    assert.ok("spec" in checkEffectText(text), effect.id);
    assert.match(text, /"size": \[\d+, \d+\]/);
  }
  assert.equal(starterText("nope"), undefined);
});

test("the effect prompt's example is Sakura and plays, and both prompts end where the person writes", () => {
  const example = EFFECT_PROMPT.slice(EFFECT_PROMPT.indexOf('{\n  "layers": [\n    {'), EFFECT_PROMPT.indexOf("\n\nAnswer with"));
  assert.deepEqual(JSON.parse(example).layers, builtinEffect("sakura")!.layers);
  for (const prompt of [EFFECT_PROMPT, DECORATION_PROMPT]) assert.ok(prompt.endsWith("What I want:\n"));
  assert.ok(DECORATION_PROMPT.includes(`${TEMPLATE_SIZE} by ${TEMPLATE_SIZE}`) && DECORATION_PROMPT.includes(`${TEMPLATE_AVATAR} pixels across`));
  assert.equal(TEMPLATE_SIZE / TEMPLATE_AVATAR, 1.2);
});

test("image layers draw the item's own pictures, by their place", () => {
  const pictures = [
    { url: "https://fuwa.test/media/leaf", animated: false },
    { url: "https://fuwa.test/media/moon", animated: true },
  ];
  const picture = { motion: "fall", phase: "idle", count: 4, from: "top", size: [20, 28], duration: [4000, 6000] };
  const json = JSON.stringify({ layers: [{ ...picture, image: 1 }, { ...picture, image: 5 }, layer] });
  const played = itemEffect({ ...effect("01JPICTURES000000000000000", json), pictures });
  assert.ok(played);
  // The layer naming a picture the item doesn't have draws nothing, not a shape.
  assert.equal(played.layers.length, 2);
  assert.equal(played.layers[0]!.image, 1);
  assert.equal(played.layers[0]!.shape, undefined);
  assert.deepEqual(played.pictures, pictures);
  // New pictures, new spec: an effect playing restarts with them.
  const fewer = itemEffect({ ...effect("01JPICTURES000000000000000", json), pictures: pictures.slice(0, 1) });
  assert.notEqual(fewer, played);
  assert.equal(fewer?.layers.length, 1);
});

test("a shader alone is an effect, and its code is kept as text that can't break anything", () => {
  const code = "fn shade(uv: vec2f) -> vec4f {\r\n  return vec4f(1.0, 0.0, 0.0, uv.y * 0.2);\u0007\n}";
  const played = itemEffect(effect("01JSHADER00000000000000000", JSON.stringify({ layers: [], shader: { code, intensity: 7, speed: 0.5 } })));
  assert.ok(played?.shader);
  assert.equal(played.layers.length, 0);
  assert.equal(played.shader.code, "fn shade(uv: vec2f) -> vec4f {\n  return vec4f(1.0, 0.0, 0.0, uv.y * 0.2);\n}");
  assert.equal(played.shader.intensity, 1);
  assert.equal(played.shader.speed, 0.5);
  assert.equal(itemEffect(effect("01JNOSHADER000000000000000", JSON.stringify({ layers: [], shader: { code: "  " } }))), null);
});

test("the shader's code is edited apart from the JSON, and put back the same", () => {
  const code = "fn shade(uv: vec2f) -> vec4f {\n  return vec4f(0.0);\n}";
  const whole = JSON.stringify({ layers: [layer], shader: { code, speed: 2 } });
  const parts = splitShader(whole);
  assert.equal(parts.wgsl, code);
  assert.deepEqual(JSON.parse(parts.json), { layers: [layer], shader: { speed: 2 } });
  assert.deepEqual(JSON.parse(joinShader(parts.json, parts.wgsl)), JSON.parse(whole));
  // No code, no shader; no JSON, a shader alone; JSON that doesn't parse stays for the checks.
  assert.deepEqual(JSON.parse(joinShader(parts.json, "")), { layers: [layer] });
  assert.deepEqual(JSON.parse(joinShader("", code)), { layers: [], shader: { code } });
  assert.equal(joinShader("{oops", code), "{oops");
  assert.deepEqual(splitShader("{oops"), { json: "{oops", wgsl: "" });
  assert.ok("spec" in checkEffectText(joinShader("", code)));
});
