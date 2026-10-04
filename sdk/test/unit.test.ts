// Unit tests for the built package (dist/), run with `pnpm test` after `pnpm build`.
import assert from "node:assert/strict";
import { test } from "node:test";
import { ConnectError, createClient, createRouterTransport } from "@connectrpc/connect";
import {
  Code,
  EventFollower,
  EventService,
  FuwaError,
  InvalidArgumentError,
  NotFoundError,
  RateLimitedError,
  UnauthenticatedError,
  UnavailableError,
  createFuwa,
  instanceUrl,
  mentions,
  parseCommand,
  retryAfterOf,
  toFuwaError,
  type Fuwa,
  type FollowUpdate,
} from "@waifu-devs/fuwa";

test("errors become typed classes", () => {
  assert.ok(toFuwaError(new ConnectError("nope", Code.NotFound)) instanceof NotFoundError);
  assert.ok(toFuwaError(new ConnectError("who", Code.Unauthenticated)).signedOut);
  assert.ok(toFuwaError(new ConnectError("bad", Code.InvalidArgument)) instanceof InvalidArgumentError);
  const slow = toFuwaError(new ConnectError("slow mode is on; you can send again in 3 seconds", Code.ResourceExhausted));
  assert.ok(slow instanceof RateLimitedError);
  assert.equal(slow.retryAfterMs, 3000);
  assert.equal(slow.retryable, true);
  const full = toFuwaError(new ConnectError("this server is full (10 members)", Code.ResourceExhausted));
  assert.equal(full.retryable, false);
  const down = toFuwaError(new ConnectError("fetch failed: connect ECONNREFUSED 10.1.2.3:443", Code.Unknown));
  assert.ok(down instanceof UnavailableError);
  assert.equal(down.network, true);
  assert.ok(!down.message.includes("10.1.2.3"), "network errors don't repeat the address");
});

test("retry-after comes from the header or slow mode's words", () => {
  assert.equal(retryAfterOf("x", new Headers({ "retry-after": "2" })), 2000);
  assert.equal(retryAfterOf("you can send again in 1 minute"), 60_000);
  assert.equal(retryAfterOf("you can send again in 2 hours"), 7_200_000);
  assert.equal(retryAfterOf("this server is out of storage"), undefined);
});

test("instance URLs: https, or http only on this computer", () => {
  assert.equal(instanceUrl("https://fuwa.chat/"), "https://fuwa.chat");
  assert.equal(instanceUrl("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
  assert.equal(instanceUrl("http://localhost:8080/"), "http://localhost:8080");
  assert.throws(() => instanceUrl("http://fuwa.chat"), /https/);
  assert.equal(instanceUrl("http://10.0.0.2:8080", true), "http://10.0.0.2:8080");
  assert.throws(() => instanceUrl("ftp://fuwa.chat"), /https/);
  assert.throws(() => instanceUrl("https://me:secret@fuwa.chat"), /password/);
});

test("commands: prefix or a mention of the agent", () => {
  assert.deepEqual(parseCommand("/dice 2d6  +1", { prefix: "/" }), { name: "dice", args: ["2d6", "+1"], rest: "2d6  +1" });
  assert.deepEqual(parseCommand("/Echo", { prefix: "/" }), { name: "echo", args: [], rest: "" });
  assert.deepEqual(parseCommand("@helper dice d20", { prefix: "/", username: "helper" }), {
    name: "dice",
    args: ["d20"],
    rest: "d20",
  });
  assert.deepEqual(parseCommand("@Helper, /ping", { prefix: "/", username: "helper" })?.name, "ping");
  assert.equal(parseCommand("hello /dice", { prefix: "/" }), undefined);
  assert.equal(parseCommand("@helpers dice", { prefix: "/", username: "helper" }), undefined);
  assert.equal(parseCommand("/ dice", { prefix: "/" }), undefined);
  assert.deepEqual(parseCommand("!roll", { prefix: "!" })?.name, "roll");
});

test("mentions match the whole @username", () => {
  assert.ok(mentions("hey @helper, roll", "helper"));
  assert.ok(mentions("@HELPER.", "helper"));
  assert.ok(mentions("ok @my.bot", "my.bot"));
  assert.ok(!mentions("hey @helpers", "helper"));
  assert.ok(!mentions("mail me@helper.com", "helper"));
  assert.ok(!mentions("hey @helper.two", "helper"));
});

/** A client whose EventService is `impl`, for testing the follower without an instance. */
function fakeEvents(impl: Record<string, unknown>): Fuwa {
  const transport = createRouterTransport(({ service }) => service(EventService, impl as never));
  return { ...createFuwa({ url: "http://localhost:1" }), events: createClient(EventService, transport) };
}

function event(serverId: string, sequence: bigint, content: string) {
  return {
    event: {
      id: `e${sequence}`,
      serverId,
      sequence,
      payload: { case: "messageCreated" as const, value: { message: { id: `m${sequence}`, serverId, content } } },
    },
  };
}

test("the follower reconnects and resumes from the last sequence", async () => {
  const asked: (bigint | undefined)[] = [];
  let calls = 0;
  const fuwa = fakeEvents({
    async *subscribe(req: { servers: { serverId: string; afterSequence?: bigint }[] }) {
      asked.push(req.servers[0]!.afterSequence);
      calls++;
      if (calls === 1) {
        yield { ready: { servers: [{ serverId: "s", sequence: 4n }] } };
        yield event("s", 5n, "a");
        yield event("s", 6n, "b");
        throw new ConnectError("restarting", Code.Unavailable);
      }
      yield event("s", 7n, "c");
      yield { ready: { servers: [{ serverId: "s", sequence: 7n }] } };
    },
  });
  const stop = new AbortController();
  const follower = new EventFollower(fuwa, { servers: ["s"], signal: stop.signal, baseDelayMs: 1, maxDelayMs: 2 });
  const seen: string[] = [];
  for await (const u of follower) {
    if (u.type === "event") seen.push(String(u.event.sequence));
    if (u.type === "disconnected") seen.push(`down:${u.error.name}`);
    if (u.type === "ready") seen.push("ready");
    if (seen.length === 6) stop.abort();
  }
  assert.deepEqual(seen, ["ready", "5", "6", "down:UnavailableError", "7", "ready"]);
  assert.deepEqual(asked, [undefined, 6n]);
  assert.equal(follower.cursors.get("s"), 7n);
});

test("the follower stops on a signed-out answer", async () => {
  const fuwa = fakeEvents({
    // eslint-disable-next-line require-yield
    async *subscribe() {
      throw new ConnectError("sign in", Code.Unauthenticated);
    },
  });
  const follower = new EventFollower(fuwa, { servers: ["s"] });
  await assert.rejects(async () => {
    for await (const _ of follower) {
      // nothing comes
    }
  }, UnauthenticatedError);
});

test("the follower notices silence", async () => {
  let calls = 0;
  const fuwa = fakeEvents({
    async *subscribe(_req: unknown, ctx: { signal: AbortSignal }) {
      calls++;
      yield { ready: { servers: [] } };
      await new Promise((resolve) => ctx.signal.addEventListener("abort", resolve));
    },
  });
  const stop = new AbortController();
  const follower = new EventFollower(fuwa, { servers: ["s"], signal: stop.signal, silenceMs: 30, baseDelayMs: 1, maxDelayMs: 2 });
  const updates: FollowUpdate["type"][] = [];
  for await (const u of follower) {
    updates.push(u.type);
    if (updates.length === 3) stop.abort();
  }
  assert.deepEqual(updates, ["ready", "disconnected", "ready"]);
  assert.equal(calls, 2);
});

test("the follower picks up new servers at once", async () => {
  const asked: string[][] = [];
  const fuwa = fakeEvents({
    async *subscribe(req: { servers: { serverId: string }[] }, ctx: { signal: AbortSignal }) {
      asked.push(req.servers.map((s) => s.serverId));
      yield { ready: { servers: [] } };
      await new Promise((resolve) => ctx.signal.addEventListener("abort", resolve));
    },
  });
  const stop = new AbortController();
  const follower = new EventFollower(fuwa, { servers: ["a"], signal: stop.signal });
  let readies = 0;
  for await (const u of follower) {
    if (u.type !== "ready") continue;
    readies++;
    if (readies === 1) follower.setServers(["a", "b"]);
    else stop.abort();
  }
  assert.deepEqual(asked, [["a"], ["a", "b"]]);
});

test("unary calls retry what's worth retrying, and only that", async () => {
  let tries = 0;
  const fuwa = createFuwa({
    url: "http://127.0.0.1:1",
    retry: { baseDelayMs: 1, maxDelayMs: 2 },
    fetch: async () => {
      tries++;
      throw new TypeError("fetch failed");
    },
  });
  // Reads are tried again after a network failure...
  await assert.rejects(fuwa.node.getNode({}), (e: FuwaError) => e instanceof UnavailableError && e.network);
  assert.equal(tries, 5);
  // ...writes aren't, since they may have gone through.
  tries = 0;
  await assert.rejects(fuwa.messages.sendMessage({ content: "hi" }), UnavailableError);
  assert.equal(tries, 1);
});

test("the token goes in the authorization header, to the instance only", async () => {
  const seen: { url: string; auth: string | null }[] = [];
  const fuwa = createFuwa({
    url: "http://127.0.0.1:9",
    token: "secret-token",
    retry: false,
    fetch: async (input, init) => {
      seen.push({ url: String(input), auth: new Headers(init?.headers).get("authorization") });
      throw new TypeError("fetch failed");
    },
  });
  await assert.rejects(fuwa.auth.getMe({}), (e: FuwaError) => !e.message.includes("secret-token"));
  assert.equal(seen.length, 1);
  assert.equal(seen[0]!.url, "http://127.0.0.1:9/fuwa.v1.AuthService/GetMe");
  assert.equal(seen[0]!.auth, "Bearer secret-token");
});
