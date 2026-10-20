import assert from "node:assert/strict";
import { test } from "node:test";
import { MIN_PER_CHANNEL, choiceText, perChannel, problems } from "../src/choose.ts";
import { idAt } from "../src/convert.ts";

test("history shrinks as a server has more channels, within bounds", () => {
  assert.equal(perChannel(5, 20_000, 1000), 1000);
  assert.equal(perChannel(40, 20_000, 1000), 500);
  assert.equal(perChannel(1000, 20_000, 1000), MIN_PER_CHANNEL);
  assert.equal(perChannel(0, 20_000, 1000), 1000);
  assert.equal(perChannel(10, 20_000, 50), 50, "never more than the most allowed");
  assert.equal(perChannel(10, 20_000, 0), 0);
});

test("a choice says what it copies, and what's wrong with it", () => {
  const choice = { parts: ["roles" as const, "history" as const], leaveOut: ["7"], messagesPerChannel: 250, days: 30 };
  assert.deepEqual(problems(choice), ["Message history needs the channels too, to go in."]);
  assert.deepEqual(problems({ ...choice, parts: [] }), ["Pick at least one thing to copy."]);
  assert.deepEqual(problems({ ...choice, parts: ["channels", "history"] }), []);
  const text = choiceText(choice);
  assert.match(text, /It copies: roles and their permissions\./);
  assert.match(text, /up to the latest 250 messages of each text channel, from the last 30 days/);
  assert.match(text, /Left out: <#7>/);
  assert.match(choiceText({ ...choice, parts: ["roles"] }), /No message history\./);
});

test("a time becomes the first Discord id made then", () => {
  // Discord's own example: 175928847299117063 was made at 1462015105796.
  assert.equal(BigInt(idAt(1_462_015_105_796)) >> 22n, BigInt("175928847299117063") >> 22n);
  assert.equal(idAt(0), "0");
});
