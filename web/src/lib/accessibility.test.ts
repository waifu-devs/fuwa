import assert from "node:assert/strict";
import { test } from "node:test";
import { applyMatrix, colorVisionMatrix, contrast, ensureContrast, feColorMatrix, highContrastTokens, seenBy, type ColorVision } from "./accessibility.ts";
import { BUILTIN_THEMES } from "./themes.ts";

const distance = (a: string, b: string) => {
  const c = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
  const [x, y] = [c(a), c(b)];
  return Math.hypot(...x.map((v, i) => v - y[i]!));
};

test("off changes nothing", () => {
  assert.equal(applyMatrix(colorVisionMatrix("off"), "#f06292"), "#f06292");
  assert.equal(applyMatrix(colorVisionMatrix("deuteranopia", 0), "#f06292"), "#f06292");
});

test("greys stay grey", () => {
  for (const kind of ["protanopia", "deuteranopia", "tritanopia"] as ColorVision[]) {
    for (const grey of ["#000000", "#808080", "#ffffff"]) assert.ok(distance(applyMatrix(colorVisionMatrix(kind), grey), grey) <= 3, `${kind} ${grey}`);
  }
});

test("the correction pulls apart colors each kind confuses", () => {
  const pairs: [ColorVision, string, string][] = [
    ["protanopia", "#e5484d", "#2fb47c"],
    ["deuteranopia", "#e5484d", "#2fb47c"],
    ["tritanopia", "#3b8beb", "#5a8a3c"],
  ];
  for (const [kind, a, b] of pairs) {
    const seen = seenBy(kind);
    const m = colorVisionMatrix(kind);
    const before = distance(applyMatrix(seen, a), applyMatrix(seen, b));
    const after = distance(applyMatrix(seen, applyMatrix(m, a)), applyMatrix(seen, applyMatrix(m, b)));
    assert.ok(after > before * 1.2, `${kind}: ${before} -> ${after}`);
  }
});

test("feColorMatrix keeps alpha", () => {
  const values = feColorMatrix(colorVisionMatrix("off")).split(" ").map(Number);
  assert.deepEqual(values, [1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0]);
});

test("ensureContrast reaches the ratio or the extreme", () => {
  assert.equal(ensureContrast("#000000", ["#ffffff"], 4.5), "#000000");
  const pink = ensureContrast("#f06292", ["#fff5f8"], 4.5);
  assert.ok(contrast(pink, "#fff5f8") >= 4.5);
  assert.equal(ensureContrast("#444444", ["#444444"], 30), "#ffffff");
  assert.equal(ensureContrast("#999999", ["#999999"], 30), "#000000");
});

test("every built-in theme meets the high contrast ratios", () => {
  for (const theme of BUILTIN_THEMES) {
    const t = highContrastTokens(theme.variant.tokens);
    const surfaces = [t.background, t.card];
    for (const s of surfaces) {
      assert.ok(contrast(t.foreground, s) >= 12, `${theme.id} text`);
      assert.ok(contrast(t["muted-foreground"], s) >= 7, `${theme.id} quiet text`);
      assert.ok(contrast(t.primary, s) >= 4.5, `${theme.id} primary`);
      assert.ok(contrast(t.border, s) >= 3, `${theme.id} border`);
    }
    assert.ok(contrast(t["primary-foreground"], t.primary) >= 4.5, `${theme.id} on primary`);
  }
});

/** The desktop's `core/accessibility.rs` pins the same values, so both apps paint alike. */
test("matches the desktop", () => {
  const shown = (i: number) => Object.values(highContrastTokens(BUILTIN_THEMES[i]!.variant.tokens)).join(" ");
  assert.equal(shown(0), "#fffafc #3b2330 #ffffff #3b2330 #ffffff #3b2330 #be4d73 #ffffff #fdecf2 #3b2330 #fce6ed #604753 #e3c9d2 #3b2330 #cf4146 #a48c95 #a48c95 #ee6191");
  assert.equal(shown(1), "#0a0910 #ece6ff #100d17 #ece6ff #100d17 #ece6ff #b388ff #13101e #312747 #ece6ff #221d34 #aea6c6 #332c46 #ece6ff #e5484d #625b76 #625b76 #b388ff");
  const samples: [ColorVision, string, string][] = [
    ["protanopia", "#e593ab", "#2f9352"],
    ["deuteranopia", "#e56295", "#2fa658"],
    ["tritanopia", "#c0554d", "#2ea57c"],
  ];
  for (const [kind, red, green] of samples) {
    assert.equal(applyMatrix(colorVisionMatrix(kind), "#e5484d"), red, kind);
    assert.equal(applyMatrix(colorVisionMatrix(kind, 0.5), "#2fb47c"), green, kind);
  }
});
