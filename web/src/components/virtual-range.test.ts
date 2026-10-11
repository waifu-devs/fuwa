import assert from "node:assert/strict";
import { test } from "node:test";
import { layoutLines, linesIn } from "./virtual-range.ts";

test("lines are laid out one after another, with the gap between them", () => {
  assert.deepEqual(layoutLines([10, 20, 30], 5), { tops: [0, 15, 40], total: 70 });
  assert.deepEqual(layoutLines([], 5), { tops: [], total: 0 });
});

test("only the lines reaching into the stretch are found", () => {
  const heights = [10, 10, 10, 10, 10];
  const { tops } = layoutLines(heights);
  assert.deepEqual(linesIn(tops, heights, 0, 9), [0, 1]);
  assert.deepEqual(linesIn(tops, heights, 15, 25), [1, 3]);
  assert.deepEqual(linesIn(tops, heights, 45, 1000), [4, 5]);
  assert.deepEqual(linesIn(tops, heights, 60, 100), [5, 5]);
  assert.deepEqual(linesIn(tops, heights, -100, -50), [0, 0]);
  assert.deepEqual(linesIn([], [], 0, 100), [0, 0]);
});

test("the search agrees with a pass over every line", () => {
  const heights = Array.from({ length: 200 }, (_, n) => 8 + ((n * 37) % 23));
  const { tops } = layoutLines(heights, 3);
  for (const [from, to] of [[0, 50], [123, 456], [999, 1001], [2000, 9000]] as const) {
    const expected = heights.map((h, n) => n).filter((n) => tops[n]! + heights[n]! >= from && tops[n]! <= to);
    const [start, end] = linesIn(tops, heights, from, to);
    assert.deepEqual(Array.from({ length: end - start }, (_, n) => start + n), expected);
  }
});
