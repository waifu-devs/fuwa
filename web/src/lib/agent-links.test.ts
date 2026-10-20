import assert from "node:assert/strict";
import { test } from "node:test";
import { agentLink, parseAgentLink } from "./agent-links.ts";

test("an agent's link is on its instance", () => {
  assert.equal(agentLink("https://fuwa.chat/", "helper"), "https://fuwa.chat/agent/helper");
  assert.equal(agentLink("https://example.com/fuwa", "a.b"), "https://example.com/fuwa/agent/a.b");
});

test("pasted links and usernames read back", () => {
  assert.deepEqual(parseAgentLink(" https://fuwa.chat/agent/Helper/?ref=1 "), { address: "https://fuwa.chat", username: "helper" });
  assert.deepEqual(parseAgentLink("example.com/fuwa/agent/@bot_1"), { address: "example.com/fuwa", username: "bot_1" });
  assert.deepEqual(parseAgentLink("@helper"), { address: "", username: "helper" });
});

test("anything else isn't one", () => {
  assert.equal(parseAgentLink("https://fuwa.chat/invite/abc"), null);
  assert.equal(parseAgentLink("/agent/helper"), null);
  assert.equal(parseAgentLink("https://fuwa.chat/agent/"), null);
  assert.equal(parseAgentLink("https://fuwa.chat/agent/no spaces"), null);
  assert.equal(parseAgentLink("helper"), null);
});
