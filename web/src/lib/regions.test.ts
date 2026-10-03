import assert from "node:assert/strict";
import { test } from "node:test";
import { hasRegions, regionMark, regionName, regionOf, sameRegion } from "./regions.ts";

const REGIONS = [
  { id: "us-east", name: "US East", home: true },
  { id: "eu", name: "Europe", home: false },
];

test("an empty region is the home region", () => {
  assert.equal(regionOf(REGIONS, "")?.id, "us-east");
  assert.equal(regionName(REGIONS, ""), "US East");
  assert.equal(regionName(REGIONS, "eu"), "Europe");
  assert.equal(regionName([], ""), "Home");
  assert.equal(regionName(REGIONS, "asia"), "asia");
});

test("there's only a choice with two regions or more", () => {
  assert.equal(hasRegions(undefined), false);
  assert.equal(hasRegions(REGIONS.slice(0, 1)), false);
  assert.equal(hasRegions(REGIONS), true);
});

test("the home region's label and empty are the same place", () => {
  assert.equal(sameRegion(REGIONS, "us-east", ""), true);
  assert.equal(sameRegion(REGIONS, "eu", ""), false);
  assert.equal(sameRegion(REGIONS, "eu", "eu"), true);
});

test("marks are two letters", () => {
  assert.equal(regionMark("US East"), "US");
  assert.equal(regionMark("South America"), "SA");
  assert.equal(regionMark("Europe"), "EU");
  assert.equal(regionMark("Asia"), "AS");
});
