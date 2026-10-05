import assert from "node:assert/strict";
import { test } from "node:test";
import { changes, flip, NO_SWITCHES, permissionPatch, switched } from "./role-edits.ts";

// Run with `pnpm test` (node's own test runner; no extra dependencies).

const MANAGE = 1 << 1;
const KICK = 1 << 2;
const BAN = 1 << 3;

test("a role saved elsewhere while it's open keeps both people's changes", () => {
  // Opened with Manage; here Ban goes on and Manage off, while someone else saves Kick.
  const mine = flip(flip(NO_SWITCHES, MANAGE, MANAGE | BAN, MANAGE), MANAGE | BAN, BAN, MANAGE);
  const now = switched(mine, MANAGE | KICK);
  assert.equal(now, KICK | BAN);
  assert.deepEqual(changes(now, MANAGE | KICK), { grant: BAN, revoke: MANAGE });
});

test("a switch flipped and put back doesn't undo someone else's change to it", () => {
  // Ban goes on and off again here, then someone else grants Ban.
  const mine = flip(flip(NO_SWITCHES, MANAGE, MANAGE | BAN, MANAGE), MANAGE | BAN, MANAGE, MANAGE);
  assert.deepEqual(changes(switched(mine, MANAGE | BAN), MANAGE | BAN), { grant: 0, revoke: 0 });
});

test("an instance without role-permission-changes gets every permission, since it ignores the switches", () => {
  assert.deepEqual(permissionPatch(KICK | BAN, KICK | MANAGE, true), { grant: BAN, revoke: MANAGE });
  assert.deepEqual(permissionPatch(KICK | BAN, KICK | MANAGE, false), { permissions: KICK | BAN });
});
