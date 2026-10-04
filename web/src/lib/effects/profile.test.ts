import assert from "node:assert/strict";
import { test } from "node:test";
import { BUILTIN_EFFECTS, introLength, isEffectId, MAX_PARTICLES, planEffect, REFERENCE, sanitizeEffect, SHAPE_PATHS } from "./profile.ts";

// Run with `pnpm test` (node's own test runner; no extra dependencies).

const card = { width: REFERENCE.width, height: REFERENCE.height, seed: "user-1" };

test("every built-in effect survives its own checks unchanged", () => {
  const ids = new Set<string>();
  for (const effect of BUILTIN_EFFECTS) {
    assert.ok(isEffectId(effect.id), effect.id);
    assert.ok(!ids.has(effect.id), `${effect.id} is listed twice`);
    ids.add(effect.id);
    assert.deepEqual(sanitizeEffect(effect), effect, effect.id);
    assert.ok(effect.layers.some((l) => l.phase === "intro"), `${effect.id} has an intro`);
    assert.ok(effect.layers.some((l) => l.phase === "idle"), `${effect.id} has an idle loop`);
  }
});

test("plans animate only transform and opacity, within the particle budget", () => {
  for (const effect of BUILTIN_EFFECTS) {
    const particles = planEffect(effect, card);
    assert.ok(particles.length > 0 && particles.length <= MAX_PARTICLES, `${effect.id}: ${particles.length}`);
    for (const p of particles) {
      assert.ok(SHAPE_PATHS[p.shape]);
      assert.ok(p.keyframes.length >= 2);
      assert.equal(p.keyframes[0]!.offset, 0);
      assert.equal(p.keyframes.at(-1)!.offset, 1);
      for (const k of p.keyframes) {
        assert.deepEqual(Object.keys(k).sort(), ["offset", "opacity", "transform"]);
        assert.ok(k.opacity >= 0 && k.opacity <= 1);
        assert.doesNotMatch(k.transform, /NaN|Infinity/);
      }
      assert.equal(p.iterations, p.phase === "intro" ? 1 : Infinity);
    }
    // The intro is short: the card should settle quickly.
    assert.ok(introLength(particles) <= 3000, `${effect.id} intro ${introLength(particles)} ms`);
  }
});

test("the same person gets the same effect every time, and others a different one", () => {
  const sakura = BUILTIN_EFFECTS[0]!;
  assert.deepEqual(planEffect(sakura, card), planEffect(sakura, card));
  assert.notDeepEqual(planEffect(sakura, card), planEffect(sakura, { ...card, seed: "user-2" }));
});

test("small cards and lite mode get fewer particles", () => {
  for (const effect of BUILTIN_EFFECTS) {
    const full = planEffect(effect, card).length;
    assert.ok(planEffect(effect, { ...card, width: 96, height: 120 }).length < full, effect.id);
    assert.ok(planEffect(effect, { ...card, lite: true }).length < full, effect.id);
    assert.ok(planEffect(effect, { ...card, lite: true }).every((p) => !p.glow));
  }
});

test("stills (reduced motion) stay out of the middle of the card", () => {
  for (const effect of BUILTIN_EFFECTS) {
    for (const p of planEffect(effect, card)) {
      if (!p.still) continue;
      const [, x, y] = /translate3d\((-?[\d.]+)px, (-?[\d.]+)px/.exec(p.still.transform)!.map(Number);
      const cx = x! + p.size / 2;
      const cy = y! + p.size / 2;
      const middle = cx > card.width * 0.2 && cx < card.width * 0.8 && cy > card.height * 0.2 && cy < card.height * 0.8;
      assert.ok(!middle, `${effect.id} still at ${cx},${cy}`);
    }
  }
});

test("specs from elsewhere are held to safe values", () => {
  assert.equal(sanitizeEffect(null), null);
  assert.equal(sanitizeEffect({ id: "Bad Id", layers: [] }), null);
  assert.equal(sanitizeEffect({ id: "empty", layers: [] }), null);
  const wild = sanitizeEffect({
    id: "wild",
    name: "x".repeat(200),
    layers: Array.from({ length: 20 }, () => ({
      shape: "star",
      motion: "twinkle",
      phase: "idle",
      from: "edges",
      count: 100000,
      size: [1, 9999],
      duration: [1, 9e9],
      colors: ["primary", "url(https://example.com/x)", "#zzzzzz", "#ff00aa", "expression(alert(1))"],
      spin: 1e9,
      sway: -5,
      extra: "ignored",
    })),
  })!;
  assert.equal(wild.name.length, 40);
  assert.equal(wild.layers.length, 6);
  for (const layer of wild.layers) {
    assert.equal(layer.count, 24);
    assert.deepEqual(layer.size, [2, 96]);
    assert.deepEqual(layer.duration, [300, 20000]);
    assert.deepEqual(layer.colors, ["primary", "#ff00aa"]);
    assert.equal(layer.spin, 1440);
    assert.equal(layer.sway, 0);
    assert.ok(!("extra" in layer));
  }
  assert.ok(planEffect(wild, card).length <= MAX_PARTICLES);
});
