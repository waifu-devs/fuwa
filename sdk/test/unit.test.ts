// Unit tests for the built package (dist/), run with `pnpm test` after `pnpm build`.
import assert from "node:assert/strict";
import { test } from "node:test";
import { ConnectError, createClient, createRouterTransport } from "@connectrpc/connect";
import {
  CallService,
  Code,
  EventFollower,
  EventService,
  FuwaError,
  CanceledError,
  InvalidArgumentError,
  NotFoundError,
  OggOpusReader,
  OggOpusWriter,
  oggOpusPackets,
  pcmFrames,
  pcmFromBytes,
  pcmToBytes,
  splitOpusPacket,
  RateLimitedError,
  UnauthenticatedError,
  UnavailableError,
  createFuwa,
  instanceUrl,
  joinVoice,
  mentions,
  opusPacketDuration,
  parseCommand,
  readOggOpus,
  retryAfterOf,
  toFuwaError,
  type Fuwa,
  type FollowUpdate,
  type Utterance,
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
  assert.equal(retryAfterOf("again in 1 minute", new Headers({ "fuwa-retry-after-ms": "1234" })), 1234);
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

test("Opus packet lengths come from their first byte", () => {
  assert.equal(opusPacketDuration(Uint8Array.of(0x08)), 20); // SILK, 20 ms, one frame
  assert.equal(opusPacketDuration(Uint8Array.of(0xfc)), 20); // CELT FB, 20 ms
  assert.equal(opusPacketDuration(Uint8Array.of(0x18)), 60); // SILK, 60 ms
  assert.equal(opusPacketDuration(Uint8Array.of(0xf9)), 40); // CELT 20 ms, two frames
  assert.equal(opusPacketDuration(Uint8Array.of(0xfb, 0x03)), 60); // CELT 20 ms, three frames
});

test("Ogg Opus files round-trip", () => {
  const writer = new OggOpusWriter({ channels: 2, serial: 7 });
  const packets = Array.from({ length: 120 }, (_, i) => Uint8Array.from({ length: 1 + (i * 37) % 600 }, (_, j) => (j ? (i + j) & 0xff : 0xfc)));
  for (const p of packets) writer.add(p);
  const file = writer.finish();
  const { head, packets: back } = readOggOpus(file);
  assert.equal(head.channels, 2);
  assert.deepEqual(back, packets);
  assert.throws(() => readOggOpus(Uint8Array.of(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28)), /Ogg/);
});

test("a voice connection survives failed rejoins", async () => {
  let listens = 0;
  let left = 0;
  const transport = createRouterTransport(({ service }) =>
    service(CallService, {
      async *listenVoice(req: { sessionId: string }, ctx: { signal: AbortSignal }) {
        listens++;
        if (listens === 2) throw new ConnectError("restarting", Code.Unavailable); // a failed rejoin
        yield { event: { case: "joined", value: { sessionId: "place-1" } } };
        if (listens === 1) {
          yield { event: { case: "frame", value: { userId: "u", opus: Uint8Array.of(0xfc, 1, 2), timestamp: 0 } } };
          throw new ConnectError("dropped", Code.Unavailable);
        }
        assert.equal(req.sessionId, "place-1", "it rejoins its own place");
        await new Promise((resolve) => ctx.signal.addEventListener("abort", resolve));
      },
      async leaveVoice() {
        left++;
        return {};
      },
    } as never),
  );
  const fuwa = { ...createFuwa({ url: "http://localhost:1" }), calls: createClient(CallService, transport) };
  const voice = await joinVoice(fuwa, { serverId: "s", channelId: "c" });
  const events: string[] = [];
  voice.on("reconnecting", () => void events.push("reconnecting"));
  await new Promise<void>((resolve) => voice.on("rejoined", () => resolve()));
  assert.deepEqual(events, ["reconnecting", "reconnecting"]);
  assert.equal(listens, 3);
  let settled = false;
  voice.closed.then(() => (settled = true), () => (settled = true));
  await new Promise((r) => setTimeout(r, 50));
  assert.equal(settled, false, "still in the channel");
  await voice.leave();
  assert.equal(left, 1);
  await voice.closed;
});

test("Opus packets of several 20 ms frames split into one each", () => {
  // Code 1: two frames of the same size.
  assert.deepEqual(splitOpusPacket(Uint8Array.of(0xf9, 1, 2, 3, 4)), [Uint8Array.of(0xf8, 1, 2), Uint8Array.of(0xf8, 3, 4)]);
  // Code 2: the first frame's size, then the rest.
  assert.deepEqual(splitOpusPacket(Uint8Array.of(0xfe, 1, 7, 8, 9)), [Uint8Array.of(0xfc, 7), Uint8Array.of(0xfc, 8, 9)]);
  // Code 3, same sizes: three frames.
  assert.deepEqual(splitOpusPacket(Uint8Array.of(0x7b, 0x03, 1, 2, 3)), [Uint8Array.of(0x78, 1), Uint8Array.of(0x78, 2), Uint8Array.of(0x78, 3)]);
  // Code 3, sizes given and two bytes of padding.
  assert.deepEqual(splitOpusPacket(Uint8Array.of(0x7b, 0xc2, 2, 1, 5, 6, 7, 0, 0)), [Uint8Array.of(0x78, 5), Uint8Array.of(0x78, 6, 7)]);
  assert.deepEqual(splitOpusPacket(Uint8Array.of(0xfc, 1)), [Uint8Array.of(0xfc, 1)]);
  assert.throws(() => splitOpusPacket(Uint8Array.of(0xf9, 1, 2, 3)), /damaged/);
  assert.throws(() => splitOpusPacket(Uint8Array.of(0xfe, 9, 1)), /damaged/);
});

test("Ogg Opus reads as it arrives, in pieces of any size", async () => {
  const writer = new OggOpusWriter();
  const packets = Array.from({ length: 80 }, (_, i) => Uint8Array.from({ length: 2 + (i * 53) % 400 }, (_, j) => (j ? i ^ j : 0xfc)));
  const pieces: Uint8Array[] = [];
  for (const [i, p] of packets.entries()) {
    writer.add(p);
    if (i % 7 === 6) {
      writer.flush();
      pieces.push(writer.take());
    }
  }
  pieces.push(writer.finish());
  const file = Uint8Array.from(pieces.flatMap((p) => [...p]));
  assert.deepEqual(readOggOpus(file).packets, packets, "taking pages as they're written makes the same file");
  for (const size of [1, 13, 300, 5000]) {
    const reader = new OggOpusReader();
    const back: Uint8Array[] = [];
    for (let at = 0; at < file.length; at += size) back.push(...reader.push(file.subarray(at, at + size)));
    reader.end();
    assert.deepEqual(back, packets, `in pieces of ${size}`);
  }
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      for (let at = 0; at < file.length; at += 999) c.enqueue(file.slice(at, at + 999));
      c.close();
    },
  });
  const streamed: Uint8Array[] = [];
  for await (const p of oggOpusPackets(stream)) streamed.push(p);
  assert.deepEqual(streamed, packets);
  await assert.rejects(async () => {
    for await (const _ of oggOpusPackets([file.subarray(0, file.length - 3)]));
  }, /middle of a page/);
});

test("PCM is cut into 20 ms frames", async () => {
  const frames: Int16Array[] = [];
  for await (const f of pcmFrames([new Int16Array(500).fill(1), new Int16Array(500).fill(2)], { sampleRate: 24_000 })) frames.push(f);
  assert.deepEqual(frames.map((f) => f.length), [480, 480, 480]);
  assert.equal(frames[1]![0], 1);
  assert.equal(frames[1]![20], 2);
  assert.equal(frames[2]![39], 2);
  assert.equal(frames[2]![40], 0, "the last frame is padded with silence");
  await assert.rejects(async () => {
    for await (const _ of pcmFrames([], { sampleRate: 44_100 as 48_000 }));
  }, /kHz/);
  const bytes = pcmToBytes(Int16Array.of(1, -2, 300, -32768));
  const back: number[] = [];
  for await (const s of pcmFromBytes([bytes.subarray(0, 3), bytes.subarray(3, 4), bytes.subarray(4)])) back.push(...s);
  assert.deepEqual(back, [1, -2, 300, -32768]);
});

/** A voice channel to test against: the test says what's heard, and sees what's said. */
function fakeVoice() {
  const heard: { userId: string; opus: Uint8Array; timestamp: number }[] = [];
  let wakeListen: (() => void) | undefined;
  const said: { frames: Uint8Array[]; at: number }[] = [];
  let queued = 0;
  let at = Date.now();
  const transport = createRouterTransport(({ service }) =>
    service(CallService, {
      async *listenVoice(_req: unknown, ctx: { signal: AbortSignal }) {
        yield { event: { case: "joined", value: { sessionId: "place" } } };
        for (;;) {
          const next = heard.shift();
          if (next) {
            yield { event: { case: "frame", value: next } };
            continue;
          }
          if (ctx.signal.aborted) return;
          await new Promise<void>((resolve) => {
            wakeListen = resolve;
            ctx.signal.addEventListener("abort", () => resolve(), { once: true });
          });
        }
      },
      async speakVoice(req: { frames: Uint8Array[] }) {
        const now = Date.now();
        queued = Math.max(0, queued - (now - at) / 20) + req.frames.length;
        at = now;
        said.push({ frames: req.frames, at: now });
        return { queued: Math.ceil(queued) };
      },
      async leaveVoice() {
        return {};
      },
    } as never),
  );
  const fuwa = { ...createFuwa({ url: "http://localhost:1" }), calls: createClient(CallService, transport) };
  let ts = 0;
  return {
    fuwa,
    said,
    hear(userId: string, opus: Uint8Array) {
      heard.push({ userId, opus, timestamp: (ts += 960) });
      wakeListen?.();
    },
  };
}

const loud = (n: number) => Uint8Array.of(0xfc, n, 1, 2, 3);
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

test("what each person says comes as utterances, as it's spoken", async () => {
  const fake = fakeVoice();
  const voice = await joinVoice(fake.fuwa, { serverId: "s", channelId: "c", utteranceGapMs: 120 });
  const got: Utterance[] = [];
  voice.on("utterance", (u) => void got.push(u));
  fake.hear("a", Uint8Array.of(0xfc)); // silence (DTX) starts nothing
  await wait(30);
  assert.equal(got.length, 0);
  for (let i = 0; i < 10; i++) {
    fake.hear("a", loud(i));
    if (i === 3) fake.hear("b", loud(99));
    await wait(5);
  }
  await wait(20);
  assert.deepEqual(got.map((u) => u.userId), ["a", "b"]);
  const a = got[0]!;
  assert.equal(a.done, false, "still talking");
  const live: number[] = [];
  const reading = (async () => {
    for await (const f of a) live.push(f.opus[1]!);
  })();
  await a.ended;
  await reading;
  assert.deepEqual(live, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  assert.equal(a.durationMs, 200);
  assert.deepEqual(readOggOpus(await a.toOgg()).packets.length, 10);
  const pieces: Uint8Array[] = [];
  for await (const piece of a.ogg({ pageMs: 60 })) pieces.push(piece);
  assert.ok(pieces.length >= 4, "the file comes in pieces");
  assert.equal(readOggOpus(Uint8Array.from(pieces.flatMap((p) => [...p]))).packets.length, 10);
  // PCM through a decoder; b's frame left a gap in a's timestamps, which comes as silence.
  const pcm: number[] = [];
  for await (const s of a.pcm({ decode: (opus) => new Int16Array(4).fill(opus[1]! + 1) })) pcm.push(s[0]!);
  assert.deepEqual(pcm, [1, 2, 3, 4, 0, 5, 6, 7, 8, 9, 10]);
  await voice.leave();
});

test("speaking sends each frame as soon as it comes, and can be talked over", async () => {
  const fake = fakeVoice();
  const voice = await joinVoice(fake.fuwa, { serverId: "s", channelId: "c" });
  // A real-time source: the first frame goes out at once, not after a batch fills.
  const started = Date.now();
  const result = await voice.speak(
    (async function* () {
      for (let i = 0; i < 6; i++) {
        yield loud(i);
        await wait(20);
      }
    })(),
  );
  assert.equal(result.interrupted, false);
  assert.equal(result.sentMs, 120);
  assert.ok(fake.said[0]!.at - started < 15, "the first frame went straight out");
  assert.deepEqual(fake.said.flatMap((s) => s.frames.map((f) => f[1])), [0, 1, 2, 3, 4, 5]);

  // Faster than real time: paced, in batches.
  fake.said.length = 0;
  const many = Array.from({ length: 100 }, (_, i) => loud(i));
  const speaking = voice.speak(many, { interruptible: (id) => id !== "bot" });
  assert.equal(voice.talking, true);
  await wait(150);
  fake.hear("bot", loud(1)); // may not cut in
  await wait(40);
  fake.hear("person", loud(1));
  const cut = await speaking;
  assert.equal(cut.interrupted, true);
  assert.equal(cut.by, "person");
  assert.ok(cut.sentMs < 1000, `stopped early (${cut.sentMs} ms sent)`);
  assert.ok(fake.said.length > 1 && fake.said.length < 20);

  // Things said together wait their turn; stopSpeaking stops them all.
  fake.said.length = 0;
  const first = voice.speak([loud(1), loud(2)]);
  const second = voice.speak([loud(3), loud(4)]);
  assert.deepEqual((await Promise.all([first, second])).map((r) => r.interrupted), [false, false]);
  assert.deepEqual(fake.said.flatMap((s) => s.frames.map((f) => f[1])), [1, 2, 3, 4]);
  const long = voice.speak(many);
  const queuedUp = voice.speak(many);
  await wait(30);
  voice.stopSpeaking();
  assert.deepEqual((await Promise.all([long, queuedUp])).map((r) => r.interrupted), [true, true]);
  assert.equal(voice.talking, false);

  const aborted = new AbortController();
  const stopped = voice.speak(many, { signal: aborted.signal });
  aborted.abort();
  await assert.rejects(stopped, CanceledError);
  await voice.leave();
  await assert.rejects(voice.speak([loud(1)]), FuwaError);
});

test("a stream of Ogg Opus plays as it arrives", async () => {
  const fake = fakeVoice();
  const voice = await joinVoice(fake.fuwa, { serverId: "s", channelId: "c" });
  const writer = new OggOpusWriter();
  writer.add(Uint8Array.of(0xf9, 0, 0)); // two 20 ms frames each
  writer.flush();
  const start = writer.take();
  for (let i = 1; i < 4; i++) writer.add(Uint8Array.of(0xf9, i, i));
  const rest = writer.finish();
  let pushMore!: () => void;
  const body = new ReadableStream<Uint8Array>({
    async start(c) {
      c.enqueue(start);
      await new Promise<void>((r) => (pushMore = r));
      c.enqueue(rest);
      c.close();
    },
  });
  const playing = voice.play({ body });
  await wait(20);
  assert.ok(fake.said.length > 0, "it started before the rest came");
  pushMore();
  const result = await playing;
  assert.equal(result.sentMs, 160);
  assert.deepEqual(fake.said.flatMap((s) => s.frames.map((f) => [f[0], f[1]])), [0, 0, 1, 1, 2, 2, 3, 3].map((n) => [0xf8, n]));
  const sixty = new OggOpusWriter();
  sixty.add(Uint8Array.of(0x18, 1));
  await assert.rejects(voice.play([sixty.finish()]), /20 ms/);
  await voice.leave();
});

test("Ogg that never finishes a packet, or never starts Opus, is refused", () => {
  // Pages of 255-byte segments that never end a packet.
  const page = (serial: number) => {
    const p = new Uint8Array(27 + 255 + 255 * 255);
    p.set([0x4f, 0x67, 0x67, 0x53]);
    new DataView(p.buffer).setUint32(14, serial, true);
    p[26] = 255;
    p.fill(255, 27, 27 + 255);
    return p;
  };
  const reader = new OggOpusReader();
  assert.throws(() => {
    for (let i = 0; i < 5; i++) reader.push(page(1));
  }, /too big/);
  // Finished packets from a stream that isn't Opus.
  const notOpus = new OggOpusReader();
  const junk = new Uint8Array(27 + 1 + 200);
  junk.set([0x4f, 0x67, 0x67, 0x53]);
  junk[26] = 1;
  junk[27] = 200;
  assert.throws(() => {
    for (let i = 0; i < 6000; i++) notOpus.push(junk);
  }, /no OpusHead/);
});

test("a stream stopped before its turn lets its download go", async () => {
  const fake = fakeVoice();
  const voice = await joinVoice(fake.fuwa, { serverId: "s", channelId: "c" });
  const first = voice.speak(Array.from({ length: 50 }, (_, i) => loud(i)));
  let cancelled = false;
  const body = new ReadableStream<Uint8Array>({
    pull() {},
    cancel() {
      cancelled = true;
    },
  });
  const second = voice.play({ body });
  await wait(30);
  voice.stopSpeaking();
  assert.deepEqual((await Promise.all([first, second])).map((r) => r.interrupted), [true, true]);
  await wait(10);
  assert.equal(cancelled, true);
  await voice.leave();
});

test("a source that isn't a stream is closed too, when its play is stopped", async () => {
  const fake = fakeVoice();
  const voice = await joinVoice(fake.fuwa, { serverId: "s", channelId: "c" });
  const first = voice.speak(Array.from({ length: 50 }, (_, i) => loud(i)));
  let closed = false;
  const source: AsyncIterable<Uint8Array> = {
    [Symbol.asyncIterator]: () => ({
      next: () => new Promise<IteratorResult<Uint8Array>>(() => {}),
      return: async () => {
        closed = true;
        return { done: true, value: undefined };
      },
    }),
  };
  const second = voice.play(source);
  await wait(30);
  voice.stopSpeaking();
  await Promise.all([first, second]);
  await wait(10);
  assert.equal(closed, true);
  await voice.leave();
});

test("an utterances() loop far behind keeps the newest 100", async () => {
  const fake = fakeVoice();
  const voice = await joinVoice(fake.fuwa, { serverId: "s", channelId: "c" });
  const loop = voice.utterances()[Symbol.asyncIterator]();
  const firstOne = loop.next(); // the loop is waiting for the first
  for (let i = 0; i < 105; i++) fake.hear(`u${i}`, loud(i));
  await wait(50);
  assert.equal(((await firstOne).value as Utterance).userId, "u0");
  // 104 more came while it wasn't reading: the four oldest were dropped.
  assert.equal(((await loop.next()).value as Utterance).userId, "u5");
  await loop.return(undefined);
  await voice.leave();
});
