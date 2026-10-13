import assert from "node:assert/strict";
import { test } from "node:test";
import { BASE_STACK, familiesOf, fontFileName, fontStack, quoteFamily, sanitizeFont, searchFamilies } from "./fonts.ts";

test("stored picks that make no sense fall back to the default", () => {
  assert.deepEqual(sanitizeFont(undefined, "app"), { kind: "default" });
  assert.deepEqual(sanitizeFont({ kind: "comic-sans" }, "app"), { kind: "default" });
  assert.deepEqual(sanitizeFont({ kind: "installed", family: "  \u0000 " }, "app"), { kind: "default" });
  assert.deepEqual(sanitizeFont({ kind: "installed", family: " Inter\n" }, "app"), { kind: "installed", family: "Inter" });
  assert.deepEqual(sanitizeFont({ kind: "file", id: "../x", name: "a.ttf" }, "app"), { kind: "default" });
  assert.deepEqual(sanitizeFont({ kind: "file", id: "abc123", name: "a.ttf" }, "code"), { kind: "file", id: "abc123", name: "a.ttf" });
  // Code's default already is the system's monospace.
  assert.deepEqual(sanitizeFont({ kind: "system" }, "app"), { kind: "system" });
  assert.deepEqual(sanitizeFont({ kind: "system" }, "code"), { kind: "default" });
});

test("a picked font comes first and the role's own after it", () => {
  assert.equal(fontStack({ kind: "default" }, "app"), BASE_STACK.app);
  assert.equal(fontStack({ kind: "default" }, "code"), BASE_STACK.code);
  assert.equal(fontStack({ kind: "installed", family: "Inter" }, "app"), `"Inter", ${BASE_STACK.app}`);
  assert.equal(fontStack({ kind: "file", id: "abc", name: "x.otf" }, "code"), `"fuwa-font-abc", ${BASE_STACK.code}`);
  assert.ok(fontStack({ kind: "system" }, "app").startsWith("system-ui"));
});

test("family names can't break out of their quotes", () => {
  assert.equal(quoteFamily('a"; color: red'), '"a\\"; color: red"');
  assert.equal(quoteFamily("a\\"), '"a\\\\"');
});

test("fonts on this computer are listed once each and searched", () => {
  const families = familiesOf([{ family: "Inter" }, { family: "Arial" }, { family: "Inter" }, { family: "" }, { family: "Noto Sans" }]);
  assert.deepEqual(families, ["Arial", "Inter", "Noto Sans"]);
  assert.deepEqual(searchFamilies(families, "n"), ["Noto Sans", "Inter"]);
  assert.deepEqual(searchFamilies(families, "  "), families);
  assert.deepEqual(searchFamilies(families, "zzz"), []);
});

test("font files are known by their names", () => {
  assert.ok(fontFileName("Inter.WOFF2"));
  assert.ok(fontFileName("x.ttf"));
  assert.ok(!fontFileName("x.png"));
});
