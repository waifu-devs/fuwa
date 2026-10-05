import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { fill, flatten, type Namespace, template } from "../../i18n/core.ts";
import type { I18n } from "../../i18n/i18n.ts";
import { sanitizeShader, shaderProblem, STARTERS, stripComments } from "./custom.ts";

/** English as the page has it (i18n/i18n.ts needs Vite, so this reads the catalog itself). */
const english = flatten({ system: JSON.parse(readFileSync(new URL("../../../../locales/en/system.json", import.meta.url), "utf8")) as Namespace });
const t: I18n["t"] = (key, values = {}) => fill(template("en", english, english, key), Object.fromEntries(Object.entries(values).map(([k, v]) => [k, String(v)])));

// Run with `pnpm test` (node's own test runner; no extra dependencies).

const SHADE = "fn shade(uv: vec2f) -> vec4f { return vec4f(0.0); }";

test("the starters pass the checks", () => {
  for (const starter of STARTERS) assert.equal(shaderProblem(t, starter.shader.code), null, starter.id);
});

test("a block comment opened inside a line comment doesn't hide the code after it", () => {
  // The security review's example: a regex that took block comments first read the binding as a comment.
  const code = `// /*\n@group(0) @binding(1) var<storage, read_write> x: array<f32>;\n// */\n${SHADE}`;
  assert.match(shaderProblem(t, code) ?? "", /attributes/);
});

test("line comments end at every WGSL line break", () => {
  for (const br of ["\n", "\r", "\v", "\f", "\u0085", " ", " "]) {
    assert.match(shaderProblem(t, `// note${br}@group(0) @binding(1) var<uniform> y: f32;\n${SHADE}`) ?? "", /attributes/, JSON.stringify(br));
    assert.match(shaderProblem(t, `// note${br}enable f16;\n${SHADE}`) ?? "", /extensions|attributes/, JSON.stringify(br));
  }
});

test("block comments nest, as WGSL's do", () => {
  assert.equal(stripComments("a /* b /* c */ d */ e"), "a   e");
  assert.match(shaderProblem(t, `/* /* */ */ @group(0) @binding(1) var<uniform> y: f32;\n${SHADE}`) ?? "", /attributes/);
  // An unclosed outer comment hides the rest, which the compiler refuses anyway.
  assert.match(shaderProblem(t, `/* /* */ ${SHADE}`) ?? "", /fn shade/);
});

test("comments can say anything but a link", () => {
  assert.equal(shaderProblem(t, `// @group /* enable\n/* @binding requires */\n${SHADE}`), null);
  assert.match(shaderProblem(t, `// from https://example.com\n${SHADE}`) ?? "", /links/);
});

test("links, extensions and attributes in code are refused", () => {
  assert.match(shaderProblem(t, `enable f16;\n${SHADE}`) ?? "", /extensions/);
  assert.match(shaderProblem(t, `@must_use ${SHADE}`) ?? "", /attributes/);
  assert.match(shaderProblem(t, "fn other() {}") ?? "", /fn shade/);
});

test("stored shaders are capped and lose control characters", () => {
  const s = sanitizeShader({ name: "x", code: `${SHADE}\r\n\u0001${"a".repeat(20000)}`, fallback: "nope" })!;
  assert.ok(new TextEncoder().encode(s.code).length <= 16 * 1024);
  assert.ok(!/[\r\u0001]/.test(s.code));
  assert.equal(s.fallback, "aurora");
});
