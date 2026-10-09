// The SDK against a real fuwa instance ($FUWA_BIN): a person sets up a server
// and an agent, and the agent answers commands, catches up after a restart and
// reconnects when the instance goes away. Run with `pnpm test:instance`.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import {
  Agent,
  ChannelType,
  Code,
  CommandOptionType,
  InteractionKind,
  type InteractionContext,
  FailedPreconditionError,
  LiveTileKind,
  MessageIntent,
  LiveTileSource,
  MediaPurpose,
  FuwaError,
  NotFoundError,
  OggOpusWriter,
  readOggOpus,
  RateLimitedError,
  UnauthenticatedError,
  createFuwa,
  messages,
  listEvents,
  type Fuwa,
  type Message,
  type Utterance,
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
  await say(`ping <@${agent.me.id}>`); // by id, as the apps write it
  const replies = await until("two replies and three mentions", async () => {
    const list = await agentMessages(agent);
    return list.length >= 2 && mentioned.length >= 3 ? list : undefined;
  });
  assert.equal(replies[0]!.content, "hello there");
  assert.equal(replies[0]!.replyToId, asked!.id);
  assert.match(replies[1]!.content, /^rolled [1-6]$/);
  assert.deepEqual(mentioned, ["@helper roll", "hey @helper, you there?", `ping <@${agent.me.id}>`]);
  // Its own replies reach the typed handler, but not "message".
  assert.ok(seen.every((s) => !s.startsWith("rolled")));
  await until("its own messages as events", () => created.some((c) => c.startsWith("rolled")) || undefined);

  const edited = await agent.edit(replies[0]!, "hello there (edited)");
  assert.equal(edited.content, "hello there (edited)");
  assert.ok(edited.editedAt);
  await agent.stop();
});

test("a live connection can carry only the messages that mention the agent", async () => {
  const agent = createFuwa({ url: instance.url, token: agentToken });
  const me = (await agent.auth.getMe({})).user!;
  const done = new AbortController();
  const stream = agent.live.open(
    { servers: [{ serverId }], messages: MessageIntent.MENTIONS },
    { signal: done.signal, timeoutMs: 0 },
  );
  const whole: string[] = [];
  const heads: string[] = [];
  let ready = () => {};
  const isReady = new Promise<void>((resolve) => (ready = resolve));
  const reading = (async () => {
    try {
      for await (const res of stream) {
        const item = res.item;
        if (item.case === "events" && item.value.ready) ready();
        if (item.case === "events" && item.value.event?.payload.case === "messageCreated") {
          whole.push(item.value.event.payload.value.message!.content);
        }
        if (item.case === "heads") heads.push(...item.value.servers.flatMap((s) => s.channels.map((c) => c.channelId)));
      }
    } catch {
      // stopped
    }
  })();
  await isReady;
  await say("not for the agent");
  await say(`for <@${me.id}>`);
  await until("the mention whole and the rest as heads", () => (whole.length && heads.length) || undefined);
  assert.deepEqual(whole, [`for <@${me.id}>`]);
  assert.deepEqual(heads, [channelId]);
  done.abort();
  await reading;
});

test("an agent reacts to messages and hears others react", async () => {
  const agent = newAgent();
  const heard: string[] = [];
  agent.on("reactionUpdated", (payload) => {
    if (payload.userId !== agent.me.id) heard.push(`${payload.added ? "+" : "-"}${payload.reaction?.emoji}${payload.reaction?.count}`);
  });
  await agent.start();
  const { message } = await say("react to this");
  const thumbs = await agent.react(message!, "👍");
  assert.equal(thumbs.count, 1);
  assert.equal(thumbs.me, true);
  await person.messages.react({ serverId, channelId, messageId: message!.id, emoji: "👍", reacted: true });
  await until("the person's reaction as an event", () => heard.includes("+👍2") || undefined);
  const who = await agent.reactors(message!, "👍");
  assert.deepEqual(
    who.map((u) => u.username),
    ["helper", "owner"],
  );
  const read = await person.messages.getMessage({ serverId, channelId, messageId: message!.id });
  assert.deepEqual(
    read.message!.reactions.map((r) => [r.emoji, r.count, r.me]),
    [["👍", 2, true]],
  );
  const off = await agent.react(message!, "👍", false);
  assert.equal(off.count, 1);
  assert.equal(off.me, false);
  await assert.rejects(agent.react(message!, "not an emoji"), (e: FuwaError) => e.code === Code.InvalidArgument);
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

test("an agent follows servers it's added to straight away", async () => {
  // No polling: the instance announces the new server on the stream.
  const agent = newAgent({ serverRefreshMs: 60_000 });
  const added: string[] = [];
  agent.on("serverAdded", (id) => void added.push(id));
  agent.command("echo", (ctx) => ctx.reply(ctx.rest));
  await agent.start();
  const { server } = await person.servers.createServer({ name: "Second" });
  const { channel } = await person.channels.createChannel({ serverId: server!.id, name: "general", type: ChannelType.TEXT });
  await person.agents.addAgent({ serverId: server!.id, username: "helper" });
  await until("serverAdded", () => added.includes(server!.id) || undefined);
  assert.ok(agent.servers.has(server!.id), "followed on the same stream");
  await say("/echo in the second server", channel!.id, server!.id);
  await until("the answer there", async () =>
    (await agentMessages(agent, channel!.id, server!.id)).some((m) => m.content === "in the second server") || undefined,
  );
  await agent.stop();
});

test("an agent answers slash commands and buttons", async () => {
  const agent = newAgent();
  const used: InteractionContext[] = [];
  agent.on("interaction", async (ctx) => {
    used.push(ctx);
    if (ctx.kind === InteractionKind.COMMAND) {
      await ctx.reply({
        content: `rolled a d${ctx.options.sides}`,
        components: [{ buttons: [{ customId: "again", label: "Roll again" }] }],
      });
    } else {
      await ctx.reply(`pressed ${ctx.customId}`);
    }
  });
  await agent.start();
  const set = await agent.setCommands(serverId, [
    {
      name: "roll",
      description: "Rolls dice",
      options: [{ name: "sides", description: "How many sides", type: CommandOptionType.INTEGER, required: true }],
    },
  ]);
  assert.deepEqual(set.map((c) => c.name), ["roll"]);

  // A member sees it and runs it; the agent answers with a button.
  const { commands, agents: owners } = await person.commands.listCommands({ serverId });
  assert.deepEqual(commands.map((c) => [c.agentId, c.command?.name]), [[agent.me.id, "roll"]]);
  assert.equal(owners[0]?.id, agent.me.id);
  const ran = await person.commands.runCommand({
    serverId,
    channelId,
    agentId: agent.me.id,
    command: "roll",
    arguments: [{ name: "sides", value: "20" }],
  });
  const answer = await until("the answer", async () =>
    (await agentMessages(agent)).find((m) => m.interaction?.id === ran.interactionId),
  );
  assert.equal(answer.content, "rolled a d20");
  assert.equal(answer.interaction?.kind, InteractionKind.COMMAND);
  assert.equal(answer.interaction?.command, "roll");
  assert.equal(answer.components[0]?.buttons[0]?.customId, "again");
  assert.equal(used[0]?.userId, (await person.auth.getMe({})).user?.id);
  assert.deepEqual(used[0]?.options, { sides: "20" });

  // Pressing the button is an interaction too.
  const pressed = await person.commands.pressButton({ serverId, messageId: answer.id, customId: "again" });
  const second = await until("the button's answer", async () =>
    (await agentMessages(agent)).find((m) => m.interaction?.id === pressed.interactionId),
  );
  assert.equal(second.content, "pressed again");
  assert.equal(used[1]?.kind, InteractionKind.BUTTON);
  assert.equal(used[1]?.messageId, answer.id);

  await agent.setCommands(serverId, []);
  await agent.stop();
});

test("an agent keeps a live tile up to date, and a webhook can too", async () => {
  const agent = newAgent();
  await agent.start();
  const tile = agent.liveTile(serverId, channelId, "final");
  const listed = async () => (await person.liveTiles.listLiveTiles({ serverId })).tiles;

  const set = await tile.set({ title: "Cup final", status: "67'", live: true, rows: [{ label: "Red Foxes", value: "2" }], progress: 0.5 });
  assert.equal(set.sourceId, agent.me.id);
  await tile.set({ title: "Cup final", status: "70'", live: true, rows: [{ label: "Red Foxes", value: "3" }] });
  const [seen] = await listed();
  assert.equal(seen?.sourceName, "Helper");
  assert.equal(seen?.sourceKind, LiveTileSource.AGENT);
  assert.deepEqual([seen?.content?.status, seen?.content?.rows[0]?.value, seen?.content?.progress], ["70'", "3", undefined]);

  // A server that turned apps' tiles off takes none and lists none.
  await person.servers.updateServer({ serverId, liveTiles: { customized: true, kinds: [LiveTileKind.VOICE, LiveTileKind.POLL] } });
  await assert.rejects(tile.set({ title: "Cup final" }), FailedPreconditionError);
  assert.deepEqual(await listed(), []);
  await person.servers.updateServer({ serverId, liveTiles: { customized: false } });

  // A webhook posts its tile over plain HTTP, into its own channel under its name.
  const { webhook } = await person.webhooks.createWebhook({ serverId, channelId, name: "Scores" });
  const address = `${instance.url}/webhooks/${serverId}/${webhook!.id}/${webhook!.token}/tile`;
  const posted = await fetch(address, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ id: "semi", title: "Semi final", status: "HT", rows: [{ label: "Blue Owls", value: "1" }] }),
  });
  assert.equal(posted.status, 204);
  const both = await listed();
  assert.deepEqual(both.map((t) => [t.sourceName, t.id]).sort(), [["Helper", "final"], ["Scores", "semi"]]);
  assert.equal((await fetch(`${address}?id=semi`, { method: "DELETE" })).status, 204);
  const wrong = await fetch(address.replace(webhook!.token, "x".repeat(64)), { method: "POST", body: JSON.stringify({ id: "a", title: "b" }) });
  assert.equal(wrong.status, 404);

  await tile.end();
  assert.deepEqual(await listed(), []);
  await person.webhooks.deleteWebhook({ serverId, webhookId: webhook!.id });
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

test("agents hold a conversation: utterances in, streamed speech out, barge-in", async () => {
  const { channel: room } = await person.channels.createChannel({ serverId, name: "Chat", type: ChannelType.VOICE });
  const otherToken = (await person.agents.createAgent({ username: "caller", displayName: "Caller" })).token;
  await person.agents.addAgent({ serverId, username: "caller" });
  const bot = newAgent();
  const caller = new Agent({ url: instance.url, token: otherToken, onError: (e) => assert.fail(e as Error) });
  agents.push(caller);
  await bot.start();
  await caller.start();
  const ear = await bot.joinVoice(serverId, room!.id, { utteranceGapMs: 300 });
  const mouth = await caller.joinVoice(serverId, room!.id);

  // The caller says something, streamed as it's made; the bot hears it as one utterance.
  const next = ear.utterances()[Symbol.asyncIterator]().next();
  const said = await mouth.speak(
    (async function* () {
      for (let i = 0; i < 15; i++) {
        yield Uint8Array.of(0xfc, i, 1, 2, 3);
        if (i % 5 === 4) await new Promise((r) => setTimeout(r, 40));
      }
    })(),
  );
  assert.equal(said.sentMs, 300);
  const utterance = (await next).value as Utterance;
  assert.equal(utterance.userId, caller.me.id);
  await utterance.ended;
  const { packets } = readOggOpus(await utterance.toOgg());
  assert.deepEqual(packets.map((p) => p[1]), Array.from({ length: 15 }, (_, i) => i));

  // The bot answers at length; the caller talks over it and it stops.
  const answer = new OggOpusWriter();
  for (let i = 0; i < 250; i++) answer.add(Uint8Array.of(0xfc, i & 0xff, 1, 2, 3)); // five seconds
  // The caller hears it, and stops hearing it as soon as it's talked over.
  let lastHeard = 0;
  const off = mouth.on("utterance", (u) => {
    if (u.userId !== bot.me.id) return;
    void (async () => {
      for await (const _ of u) lastHeard = Date.now();
    })();
  });
  const started = Date.now();
  const answering = ear.play(new Blob([answer.finish()]).stream(), { interruptible: true });
  await new Promise((r) => setTimeout(r, 400));
  await mouth.speak([Uint8Array.of(0xfc, 1, 1, 2, 3), Uint8Array.of(0xfc, 2, 1, 2, 3)]);
  const result = await answering;
  assert.equal(result.interrupted, true);
  assert.equal(result.by, caller.me.id);
  assert.ok(Date.now() - started < 2500, "it stopped well before the end");
  const stoppedAt = Date.now();
  await new Promise((r) => setTimeout(r, 500));
  off();
  assert.ok(lastHeard > started, "the caller heard the answer");
  // What the instance had queued (up to 200 ms) is dropped, not played out.
  assert.ok(lastHeard - stoppedAt < 100, `it went quiet at once (${lastHeard - stoppedAt} ms after)`);

  await mouth.leave();
  await ear.leave();
  await caller.stop();
  await bot.stop();
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
