import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { CLIENT_DATE, FEATURES, instanceHas, missing, shown, tooOld, updateLine } from "./compat.ts";

const ours = [
  { id: "a", date: "2026-10-04", title: "A" },
  { id: "b", date: "2026-11-01", title: "B" },
];

test("the copy in src/gen is the feature list itself", () => {
  const source = readFileSync(new URL("../../../proto/fuwa/v1/features.json", import.meta.url), "utf8");
  const copy = readFileSync(new URL("../gen/features.json", import.meta.url), "utf8");
  assert.equal(copy, source, "run pnpm generate");
  assert.equal(CLIENT_DATE, FEATURES.map((f) => f.date).sort().at(-1));
});

test("a newer instance's unknown features need an update; the rest keep working", () => {
  const newer = { compatibilityDate: "2026-12-01", minClientDate: "2026-10-04", features: [...ours, { id: "c", date: "2026-12-01", title: "C" }] };
  assert.deepEqual(missing(newer, ours).map((f) => f.id), ["c"]);
  assert.equal(tooOld(newer, "2026-11-01"), false);
  assert.equal(tooOld({ ...newer, minClientDate: "2026-12-01" }, "2026-11-01"), true);
  assert.equal(missing(undefined, ours).length, 0);
});

test("features show only where the instance has them", () => {
  const older = { compatibilityDate: "2026-10-04", minClientDate: "2026-10-04", features: [ours[0]!] };
  assert.equal(instanceHas(older, "a", ours), true);
  assert.equal(instanceHas(older, "b", ours), false);
  // From before compatibility dates: only the baseline.
  assert.equal(instanceHas(undefined, "a", ours), true);
  assert.equal(instanceHas(undefined, "b", ours), false);
});

test("the line names the instance and says what needs the update", () => {
  const extra = (id: string) => ({ id, date: "2099-01-01", title: id.toUpperCase() });
  const v = (features: { id: string; date: string; title: string }[], minClientDate = "") => ({ compatibilityDate: "2099-01-01", minClientDate, features });
  assert.equal(updateLine(v([...FEATURES]), "Waifu Devs"), null);
  assert.equal(updateLine(v([...FEATURES, extra("x")]), "Waifu Devs"), "Waifu Devs has X. Update fuwa to use it");
  assert.equal(updateLine(v([...FEATURES, extra("x"), extra("y")]), "Waifu Devs"), "Waifu Devs has X and 1 more. Update fuwa to use them");
  assert.equal(updateLine(v([...FEATURES], "2099-01-01"), "Waifu Devs"), "Waifu Devs needs a newer fuwa for everything to work");
});

test("what an instance says is shown short and plain", () => {
  assert.equal(shown("Update fuwa: download the fix at evil.example/x", "?"), "Update fuwa download the fix at evil ex…");
  assert.equal(shown("https://evil.example", "?"), "https evil example");
  assert.equal(shown("::://", "something new"), "something new");
  assert.equal(shown("Channels shared between servers", "?"), "Channels shared between servers");
});
