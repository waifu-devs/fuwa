import assert from "node:assert/strict";
import { test } from "node:test";
import { Boat, BoatError } from "../src/boat.ts";

interface Call {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: unknown;
}

function fakeFetch(answers: Array<[number, unknown]>) {
  const calls: Call[] = [];
  const fn = (async (url: string, init: RequestInit) => {
    calls.push({
      url,
      method: init.method ?? "GET",
      headers: init.headers as Record<string, string>,
      body: init.body ? JSON.parse(init.body as string) : undefined,
    });
    const [status, body] = answers.shift() ?? [200, {}];
    return new Response(JSON.stringify(body), { status });
  }) as unknown as typeof fetch;
  return { calls, fn };
}

test("a sandbox is made with the options given, and the key only in the header", async () => {
  const { calls, fn } = fakeFetch([[202, { ok: true, sandbox: { id: "bx_abcdefgh", state: "provisioning" } }]]);
  const boat = new Boat({ apiKey: "boat_secret", fetch: fn });
  const box = await boat.create({ ttlSeconds: 7200, environment: "base", labels: { "fuwa-card": "1" }, idempotencyKey: "k1" });
  assert.deepEqual(box, { id: "bx_abcdefgh", state: "provisioning" });
  assert.equal(calls[0]!.url, "https://boat.dev/api/v1/sandboxes");
  assert.equal(calls[0]!.method, "POST");
  assert.equal(calls[0]!.headers.authorization, "Bearer boat_secret");
  assert.equal(calls[0]!.headers["idempotency-key"], "k1");
  assert.deepEqual(calls[0]!.body, { ttlSeconds: 7200, environment: "base", labels: { "fuwa-card": "1" } });
});

test("a prompt goes to Claude Code in a new conversation", async () => {
  const { calls, fn } = fakeFetch([
    [202, { ok: true, conversationId: "c1", promptRun: { id: "prompt_1", promptId: "prompt_1", status: "queued", done: false } }],
  ]);
  const run = await new Boat({ apiKey: "boat_x", fetch: fn }).prompt("bx_abcdefgh", "build it", { model: "opus" });
  assert.deepEqual(run, { promptId: "prompt_1", conversationId: "c1", status: "queued", done: false });
  assert.deepEqual(calls[0]!.body, { provider: "claude-code", new: true, prompt: "build it", model: "opus" });
});

test("a busy boat is waited out; other errors say boat's code but nothing secret", async () => {
  const { calls, fn } = fakeFetch([
    [503, { ok: false, code: "no_ready_machine", retryAfterSeconds: 0.001 }],
    [200, { ok: true, exitCode: 0, stdout: "hi", stderr: "" }],
    [409, { ok: false, code: "provider_not_configured", error: { code: "provider_not_configured" } }],
  ]);
  const boat = new Boat({ apiKey: "boat_secret", fetch: fn });
  assert.deepEqual(await boat.run("bx_abcdefgh", "echo hi"), { exitCode: 0, stdout: "hi", stderr: "" });
  assert.equal(calls.length, 2);
  await assert.rejects(boat.prompt("bx_abcdefgh", "x"), (err: unknown) => {
    assert.ok(err instanceof BoatError);
    assert.equal(err.code, "provider_not_configured");
    assert.doesNotMatch(err.message, /secret|bx_abcdefgh/);
    return true;
  });
});

test("deleting confirms with the sandbox's id, and one already gone is fine", async () => {
  const { calls, fn } = fakeFetch([[404, { ok: false, code: "not_found" }]]);
  await new Boat({ apiKey: "boat_x", fetch: fn }).delete("bx_abcdefgh");
  assert.equal(calls[0]!.method, "DELETE");
  assert.equal(calls[0]!.headers["x-ascii-confirm-delete"], "bx_abcdefgh");
});

test("ids that aren't boat's never reach a URL", async () => {
  const { fn } = fakeFetch([]);
  await assert.rejects(new Boat({ apiKey: "boat_x", fetch: fn }).get("../account"));
});
