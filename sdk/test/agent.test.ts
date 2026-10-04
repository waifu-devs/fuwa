// The SDK against a real fuwa instance ($FUWA_BIN): a person sets up a server
// and an agent, and the agent answers commands, catches up after a restart and
// reconnects when the instance goes away. Run with `pnpm test:instance`.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import {
  Agent,
  ChannelType,
  Code,
  MediaPurpose,
  FuwaError,
  NotFoundError,
  OggOpusWriter,
  RateLimitedError,
  UnauthenticatedError,
  createFuwa,
  messages,
  listEvents,
  type Fuwa,
  type Message,
  type VoiceFrame,
} from "@waifu-devs/fuwa";
import { startInstance, until, type Instance } from "./instance.ts";

let instance: Instance;
let person: Fuwa;
let serverId: string;
let channelId: string;
let agentToken: string;
const agents: Agent[] = [];

// The smallest valid PNG: one transparent pixel.
const PIXEL = Uint8Array.from(
  atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="),
  (c) => c.charCodeAt(0),
);

function newAgent(extra: Partial<ConstructorParameters<typeof Agent>[0]> = {}): Agent {
  const agent = new Agent({ url: instance.url, token: agentToken, onError: (e) => assert.fail(e as Error), ...extra });
  agents.push(agent);
  return agent;
}

/** The agent's messages in the channel, oldest first. */
async function agentMessages(agent: Agent, channel = channelId, server = serverId): Promise<Message[]> {
  const out: Message[] = [];
  for await (const { message } of messages(person, { serverId: server, channelId: channel })) {
    if (message.authorId === agent.me.id) out.unshift(message);
  }
  return out;
}

function say(content: string, channel = channelId, server = serverId) {
  return person.messages.sendMessage({ serverId: server, channelId: channel, content });
}

before(async () => {
  instance = await startInstance();
  const anonymous = createFuwa({ url: instance.url });
  const { token } = await anonymous.auth.signUp({ username: "owner", password: "correct horse battery" });
  person = createFuwa({ url: instance.url, token });
  const { server } = await person.servers.createServer({ name: "Agents" });
  serverId = server!.id;
  const { channel } = await person.channels.createChannel({ serverId, name: "bots", type: ChannelType.TEXT });
  channelId = channel!.id;
  const made = await person.agents.createAgent({ username: "helper", displayName: "Helper" });
  agentToken = made.token;
  await person.agents.addAgent({ serverId, username: "helper" });
});

after(async () => {
  await Promise.all(agents.map((a) => a.stop()));
  await instance?.stop();
});

test("the instance says what it runs", async () => {
  const { version, node } = await person.serverVersion();
  assert.match(version, /^\d+\.\d+\.\d+/);
  assert.equal(node.publicUrl, instance.url);
});

test("a wrong token fails to start, as UnauthenticatedError", async () => {
  const agent = new Agent({ url: instance.url, token: "not-a-token" });
  await assert.rejects(agent.start(), UnauthenticatedError);
});

test("an agent answers commands, mentions and messages", async () => {
  const agent = newAgent();
  const mentioned: string[] = [];
  const seen: string[] = [];
  const created: string[] = [];
  agent.command("echo", (ctx) => ctx.reply(ctx.rest || "(nothing)"), { description: "Says it back" });
  agent.command("dice", (ctx) => ctx.reply(`rolled ${1 + Math.floor(Math.random() * 6)}`), { aliases: ["roll"] });
  agent.on("mention", (ctx) => void mentioned.push(ctx.content));
  agent.on("message", (ctx) => void seen.push(ctx.content));
  agent.on("messageCreated", (payload) => void created.push(payload.message?.content ?? ""));
  await agent.start();
  assert.equal(agent.me.username, "helper");
  assert.deepEqual(
    agent.commands.map((c) => c.name),
    ["echo", "dice"],
  );

  const { message: asked } = await say("/echo hello there");
  await say("@helper roll");
  await say("hey @helper, you there?");
  const replies = await until("three replies... or two and a mention", async () => {
    const list = await agentMessages(agent);
    return list.length >= 2 && mentioned.length >= 2 ? list : undefined;
  });
  assert.equal(replies[0]!.content, "hello there");
  assert.equal(replies[0]!.replyToId, asked!.id);
  assert.match(replies[1]!.content, /^rolled [1-6]$/);
  assert.deepEqual(mentioned, ["@helper roll", "hey @helper, you there?"]);
  // Its own replies reach the typed handler, but not "message".
  assert.ok(seen.every((s) => !s.startsWith("rolled")));
  await until("its own messages as events", () => created.some((c) => c.startsWith("rolled")) || undefined);

  const edited = await agent.edit(replies[0]!, "hello there (edited)");
  assert.equal(edited.content, "hello there (edited)");
  assert.ok(edited.editedAt);
  await agent.stop();
});

test("an agent catches up on what it missed while stopped", async () => {
  const first = newAgent();
  await first.start();
  await say("before");
  await until("the event", () => (first.cursors.get(serverId) ?? 0n) > 0n || undefined);
  await first.stop();
  const cursors = Object.fromEntries(first.cursors);

  await say("/echo while you were away");
  const second = newAgent({ cursors });
  second.command("echo", (ctx) => ctx.reply(ctx.rest));
  await second.start();
  await until("the missed command answered", async () =>
    (await agentMessages(second)).some((m) => m.content === "while you were away") || undefined,
  );
  await second.stop();
});

test("an agent reconnects when the instance restarts, and misses nothing", async () => {
  const agent = newAgent();
  const drops: string[] = [];
  agent.on("disconnected", ({ error }) => void drops.push(error.name));
  agent.command("echo", (ctx) => ctx.reply(ctx.rest));
  await agent.start();
  await instance.restart();
  await say("/echo after the restart");
  await until(
    "the answer after reconnecting",
    async () => (await agentMessages(agent)).some((m) => m.content === "after the restart") || undefined,
    30_000,
  );
  assert.ok(drops.length >= 1, "it noticed the stream break");
  await agent.stop();
});

test("an agent notices servers it's added to", async () => {
  const agent = newAgent({ serverRefreshMs: 100 });
  const added: string[] = [];
  agent.on("serverAdded", (id) => void added.push(id));
  agent.command("echo", (ctx) => ctx.reply(ctx.rest));
  await agent.start();
  const { server } = await person.servers.createServer({ name: "Second" });
  const { channel } = await person.channels.createChannel({ serverId: server!.id, name: "general", type: ChannelType.TEXT });
  await person.agents.addAgent({ serverId: server!.id, username: "helper" });
  await until("serverAdded", () => added.includes(server!.id) || undefined);
  await until("the stream following it", () => agent.servers.has(server!.id) || undefined);
  // Give the new stream a moment to be live, then talk there.
  await new Promise((r) => setTimeout(r, 300));
  await say("/echo in the second server", channel!.id, server!.id);
  await until("the answer there", async () =>
    (await agentMessages(agent, channel!.id, server!.id)).some((m) => m.content === "in the second server") || undefined,
  );
  await agent.stop();
});

test("slow mode is waited out; other failures are typed", async () => {
  await person.channels.updateChannel({ serverId, channelId, slowmodeSeconds: 1 });
  const agent = newAgent();
  await agent.start();
  const started = Date.now();
  await agent.send(serverId, channelId, "one");
  await agent.send(serverId, channelId, "two");
  assert.ok(Date.now() - started >= 900, "the second send waited for slow mode");

  const impatient = createFuwa({ url: instance.url, token: agentToken, retry: false });
  const err = await impatient.messages.sendMessage({ serverId, channelId, content: "three" }).catch((e) => e);
  assert.ok(err instanceof RateLimitedError);
  assert.ok(err.retryAfterMs! > 0 && err.retryAfterMs! <= 1000);
  await person.channels.updateChannel({ serverId, channelId, slowmodeSeconds: 0 });

  await assert.rejects(agent.send(serverId, "no-such-channel", "hi"), NotFoundError);
  await agent.stop();
});

test("pictures upload through the instance", async () => {
  const agent = newAgent();
  await agent.start();
  const media = await agent.upload({ purpose: MediaPurpose.AVATAR, data: PIXEL, contentType: "image/png" });
  assert.ok(media.url.startsWith(`${instance.url}/media/`));
  const res = await fetch(media.url);
  assert.equal(res.status, 200);
  assert.deepEqual(new Uint8Array(await res.arrayBuffer()), PIXEL);
  await agent.stop();
});

test("pages walk a channel both ways", async () => {
  const { channel } = await person.channels.createChannel({ serverId, name: "paging", type: ChannelType.TEXT });
  const sent: string[] = [];
  for (let i = 0; i < 7; i++) sent.push((await say(`m${i}`, channel!.id)).message!.id);
  const back: string[] = [];
  for await (const { message, author } of messages(person, { serverId, channelId: channel!.id, pageSize: 3 })) {
    back.push(message.id);
    assert.equal(author?.username, "owner");
  }
  assert.deepEqual(back, [...sent].reverse());
  const forward: string[] = [];
  for await (const { message } of messages(person, {
    serverId,
    channelId: channel!.id,
    direction: "newer",
    from: sent[0],
    pageSize: 2,
  })) {
    forward.push(message.id);
  }
  assert.deepEqual(forward, sent.slice(1));
  let count = 0;
  for await (const _ of listEvents(person, serverId, { pageSize: 5 })) count++;
  assert.ok(count >= 7);
});

test("agents hear and talk in voice channels", async () => {
  const { channel: voice } = await person.channels.createChannel({ serverId, name: "Lounge", type: ChannelType.VOICE });
  const listenerToken = (await person.agents.createAgent({ username: "listener", displayName: "Listener" })).token;
  await person.agents.addAgent({ serverId, username: "listener" });
  const talker = newAgent();
  const listener = new Agent({ url: instance.url, token: listenerToken, onError: (e) => assert.fail(e as Error) });
  agents.push(listener);
  await talker.start();
  await listener.start();

  const ear = await listener.joinVoice(serverId, voice!.id);
  const mouth = await talker.joinVoice(serverId, voice!.id);
  assert.ok(mouth.sessionId);
  assert.equal(mouth.state?.channelId, voice!.id);
  const { states } = await person.calls.listVoiceStates({ serverId });
  assert.deepEqual(new Set(states.map((s) => s.userId)), new Set([talker.me.id, listener.me.id]));

  const talk: string[] = [];
  ear.on("speaking", (id) => void talk.push(`start:${id}`));
  ear.on("silent", (id) => void talk.push(`stop:${id}`));
  const heard: VoiceFrame[] = [];
  const reading = (async () => {
    for await (const frame of ear) {
      heard.push(frame);
      if (heard.length === 25) return;
    }
  })();

  // An Ogg Opus file of 25 frames: 20 ms each (0xfc), numbered.
  const file = new OggOpusWriter();
  for (let i = 0; i < 25; i++) file.add(Uint8Array.of(0xfc, i, 1, 2, 3));
  const started = Date.now();
  await mouth.play(file.finish());
  assert.ok(Date.now() - started >= 300, "it plays about as fast as the sound lasts");
  await reading;
  assert.ok(heard.every((f) => f.userId === talker.me.id));
  assert.deepEqual(
    heard.map((f) => f.opus[1]),
    Array.from({ length: 25 }, (_, i) => i),
  );
  assert.equal((heard[1]!.timestamp - heard[0]!.timestamp) >>> 0, 960);
  await until("stopped speaking", () => talk.includes(`stop:${talker.me.id}`) || undefined, 3000);
  assert.equal(talk[0], `start:${talker.me.id}`);

  // 60 ms frames don't fit a voice channel.
  const long = new OggOpusWriter();
  long.add(Uint8Array.of(0x18, 1, 2, 3));
  await assert.rejects(mouth.play(long.finish()), /20 ms/);

  const muted = await mouth.setState({ selfMute: true });
  assert.equal(muted?.selfMute, true);

  // A moderator takes the listener out: that's final.
  await person.calls.moderateVoice({ serverId, userId: listener.me.id, disconnect: true });
  const ended = await ear.closed.then(
    () => undefined,
    (e) => e,
  );
  assert.ok(ended instanceof FuwaError && ended.code === Code.FailedPrecondition, String(ended));

  await mouth.leave();
  await mouth.closed;
  await until("nobody left in voice", async () =>
    (await person.calls.listVoiceStates({ serverId })).states.length === 0 || undefined,
  );
  await listener.stop();
  await talker.stop();
});

test("a voice connection rejoins after the instance restarts", async () => {
  const { channel: voice } = await person.channels.createChannel({ serverId, name: "Stage", type: ChannelType.VOICE });
  const agent = newAgent();
  await agent.start();
  const conn = await agent.joinVoice(serverId, voice!.id);
  const session = conn.sessionId;
  const events: string[] = [];
  conn.on("reconnecting", () => void events.push("reconnecting"));
  conn.on("rejoined", () => void events.push("rejoined"));
  await instance.restart();
  await until("rejoined", () => events.includes("rejoined") || undefined, 20_000);
  assert.equal(conn.sessionId, session, "it keeps its place");
  await conn.speak([Uint8Array.of(0xfc, 9, 9, 9)]);
  await agent.stop();
  await assert.doesNotReject(conn.closed);
});

test("a reset token stops a running agent with UnauthenticatedError", async () => {
  const agent = newAgent({ onError: () => {} });
  await agent.start();
  const { agents: mine } = await person.agents.listAgents({});
  const { token } = await person.agents.resetAgentToken({ agentId: mine[0]!.user!.id });
  agentToken = token;
  await assert.rejects(agent.closed, UnauthenticatedError);
});
