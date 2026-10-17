import assert from "node:assert/strict";
import { test } from "node:test";
import { CODE_MS, Links, newCode, normalize, type Pending } from "../src/link.ts";

const pending = (guildId: string): Pending => ({ guildId, channelId: "c", userId: "u", messagesPerChannel: 10 });

test("codes are easy to type back", () => {
  assert.match(newCode(), /^[A-HJKMNP-Z2-9]{4}-[A-HJKMNP-Z2-9]{4}$/);
  assert.equal(normalize(" abcd efgh "), "ABCD-EFGH");
  assert.equal(normalize("abcd-efgh"), "ABCD-EFGH");
});

test("a code works once, in any case", () => {
  const links = new Links();
  const code = links.create(pending("g1"));
  const used = links.redeem(code.toLowerCase(), "s1");
  assert.ok(used.ok && used.pending.guildId === "g1");
  assert.deepEqual(links.redeem(code, "s1"), { ok: false, why: "wrong" });
});

test("a new code for a guild replaces its old one, and codes run out", () => {
  let now = 0;
  const links = new Links(() => now);
  const first = links.create(pending("g1"));
  const second = links.create(pending("g1"));
  assert.equal(links.redeem(first, "s1").ok, false);
  now += CODE_MS;
  assert.equal(links.redeem(second, "s1").ok, false);
});

test("a server gets a few wrong tries, then waits", () => {
  let now = 0;
  const links = new Links(() => now);
  const code = links.create(pending("g1"));
  for (let i = 0; i < 5; i++) assert.deepEqual(links.redeem("WRONG-CODE", "s1"), { ok: false, why: "wrong" });
  assert.deepEqual(links.redeem(code, "s1"), { ok: false, why: "slow down" });
  // Other servers aren't held back, and the wait ends.
  assert.deepEqual(links.redeem("WRONG-CODE", "s2"), { ok: false, why: "wrong" });
  now += 10 * 60_000;
  assert.equal(links.redeem(code, "s1").ok, true);
});
