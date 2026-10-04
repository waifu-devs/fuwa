import assert from "node:assert/strict";
import { test } from "node:test";
import { FRESH, entryOf, look } from "./fresh.ts";

const INDEX = (name: string) =>
  `<!doctype html><html><head><script>window.x=1</script><script type="module" crossorigin src="/assets/${name}.js"></script>` +
  `<link rel="modulepreload" crossorigin href="/assets/vendor-1.js"></head><body><div id="root"></div></body></html>`;

test("the entry script is the module one", () => {
  assert.equal(entryOf(INDEX("index-abc")), "/assets/index-abc.js");
  assert.equal(entryOf(`<script src="/a.js" type='module'>`), "/a.js");
  assert.equal(entryOf("<script src=\"/plain.js\"></script>"), null);
  assert.equal(entryOf("not html at all"), null);
});

test("a new build has to be seen twice before the page is behind", () => {
  const ours = "/assets/index-old.js";
  let s = look(FRESH, ours, "/assets/index-new.js");
  assert.equal(s.stale, false);
  s = look(s, ours, "/assets/index-new.js");
  assert.equal(s.stale, true);
  // Once behind, a look that fails doesn't change it.
  assert.equal(look(s, ours, null).stale, true);
});

test("a rolling deploy doesn't flap", () => {
  const ours = "/assets/index-old.js";
  let s = look(FRESH, ours, "/assets/index-new.js");
  s = look(s, ours, ours);
  assert.deepEqual(s, FRESH);
  s = look(s, ours, "/assets/index-new.js");
  assert.equal(s.stale, false);
  // Two different new builds in a row start the count again.
  s = look(s, ours, "/assets/index-newer.js");
  assert.equal(s.stale, false);
  assert.equal(look(s, ours, "/assets/index-newer.js").stale, true);
});
