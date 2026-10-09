import assert from "node:assert/strict";
import { test } from "node:test";
import { groupOf, grouped, offered, readable, sameEvents, toggle, toggleAll } from "./agent-events.ts";

// Run with `pnpm test` (node's own test runner; no extra dependencies).

const NAMES = ["server_updated", "message_created", "message_deleted", "member_joined", "interaction_created", "voice_state_updated", "reaction_updated", "live_tile_ended"];

test("events that are never delivered aren't offered, unless already chosen", () => {
  assert.deepEqual(offered(NAMES), ["server_updated", "message_created", "message_deleted", "member_joined", "interaction_created", "reaction_updated"]);
  assert.ok(offered(NAMES, ["voice_state_updated"]).includes("voice_state_updated"));
  assert.ok(offered(NAMES, ["something_new"]).includes("something_new"));
});

test("names fall into groups in a fixed order, the rest under other", () => {
  assert.equal(groupOf("message_created"), "messages");
  assert.equal(groupOf("interaction_created"), "interactions");
  assert.equal(groupOf("member_left"), "members");
  assert.equal(groupOf("reactions_cleared"), "reactions");
  assert.equal(groupOf("emojis_updated"), "other");
  assert.deepEqual(grouped(offered(NAMES)), [
    { group: "messages", names: ["message_created", "message_deleted"] },
    { group: "interactions", names: ["interaction_created"] },
    { group: "members", names: ["member_joined"] },
    { group: "reactions", names: ["reaction_updated"] },
    { group: "other", names: ["server_updated"] },
  ]);
  assert.deepEqual(grouped(["member_joined"]), [{ group: "members", names: ["member_joined"] }]);
});

test("a name reads as words", () => {
  assert.equal(readable("shared_channels_updated"), "Shared channels updated");
  assert.equal(readable(""), "");
});

test("toggling one name or a whole group", () => {
  assert.deepEqual(toggle([], "message_created"), ["message_created"]);
  assert.deepEqual(toggle(["message_created", "member_joined"], "message_created"), ["member_joined"]);
  assert.deepEqual(toggleAll(["message_created"], ["message_created", "message_deleted"]), ["message_created", "message_deleted"]);
  assert.deepEqual(toggleAll(["member_joined", "message_created", "message_deleted"], ["message_created", "message_deleted"]), ["member_joined"]);
});

test("the same events in any order are the same choice", () => {
  assert.ok(sameEvents(["a", "b"], ["b", "a"]));
  assert.ok(!sameEvents(["a"], ["a", "b"]));
  assert.ok(sameEvents([], []));
});
