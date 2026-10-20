import assert from "node:assert/strict";
import { test } from "node:test";
import { Permission } from "@waifu-devs/fuwa";
import { IMPORTER_NAME, NotReadyError, migrate, newReport } from "../src/migrate.ts";
import {
  Refused,
  type Download,
  type Hook,
  type Source,
  type SourceGuild,
  type SourceMessage,
  type Target,
  type TargetFile,
  type TargetOverwrite,
  type TargetPost,
} from "../src/model.ts";
import { MemoryStore } from "../src/state.ts";
import { progressText, summaryText } from "../src/summary.ts";

const DAY = 24 * 60 * 60_000;
const T0 = Date.UTC(2024, 0, 1, 12);

function message(id: string, extra: Partial<SourceMessage> = {}): SourceMessage {
  return {
    id,
    authorName: "alice",
    authorAvatarUrl: "https://cdn.discordapp.com/avatars/1/a.png",
    at: T0,
    content: `message ${id}`,
    system: false,
    pinned: false,
    attachments: [],
    embeds: [],
    stickers: [],
    mentionedUsers: {},
    reactions: [],
    ...extra,
  };
}

const guild: SourceGuild = {
  id: "g",
  name: "Cozy Corner",
  description: "a nice place",
  iconUrl: "https://cdn/icon.png",
  roles: [
    {
      id: "g",
      name: "@everyone",
      color: 0,
      permissions: (1n << 10n) | (1n << 11n),
      hoist: false,
      mentionable: false,
      position: 0,
      managed: false,
      everyone: true,
    },
    {
      id: "r-admin",
      name: "Admin",
      color: 0xff0000,
      permissions: 1n << 3n,
      hoist: true,
      mentionable: false,
      position: 3,
      managed: false,
      everyone: false,
    },
    {
      id: "r-bot",
      name: "Some Bot",
      color: 0,
      permissions: 0n,
      hoist: false,
      mentionable: false,
      position: 2,
      managed: true,
      everyone: false,
    },
    {
      id: "r-mod",
      name: "Mod",
      color: 0x00ff00,
      permissions: 1n << 13n,
      hoist: false,
      mentionable: true,
      position: 1,
      managed: false,
      everyone: false,
    },
  ],
  channels: [
    { id: "c-cat", name: "Chat", kind: "category", position: 0, topic: "", slowmodeSeconds: 0, overwrites: [] },
    {
      id: "c-general",
      name: "general",
      kind: "text",
      parentId: "c-cat",
      position: 0,
      topic: "hello",
      slowmodeSeconds: 5,
      overwrites: [],
    },
    {
      id: "302",
      name: "mods",
      kind: "text",
      parentId: "c-cat",
      position: 1,
      topic: "",
      slowmodeSeconds: 0,
      overwrites: [
        { id: "g", kind: "role", allow: 0n, deny: 1n << 10n },
        { id: "r-mod", kind: "role", allow: 1n << 10n, deny: 0n },
      ],
    },
    {
      id: "c-voice",
      name: "Lounge",
      kind: "voice",
      parentId: "c-cat",
      position: 0,
      topic: "",
      slowmodeSeconds: 0,
      overwrites: [],
    },
    { id: "c-forum", name: "help", kind: "forum", position: 1, topic: "", slowmodeSeconds: 0, overwrites: [] },
    { id: "c-dir", name: "directory", kind: "other", position: 2, topic: "", slowmodeSeconds: 0, overwrites: [] },
  ],
  emojis: [{ id: "400", name: "wave", url: "https://cdn/wave.png", animated: false }],
  stickers: [
    { id: "s-cat", name: "cat", url: "https://cdn/cat.png", format: "png" },
    { id: "s-lottie", name: "dance", url: "https://cdn/dance.json", format: "lottie" },
  ],
  sounds: [
    { id: "snd-1", name: "airhorn", url: "https://cdn/airhorn.mp3", volume: 0.5, emojiId: "400", emojiName: "wave" },
    { id: "snd-2", name: "Airhorn", url: "https://cdn/airhorn2.mp3", volume: 1, emojiName: "📯" },
  ],
};

class FakeSource implements Source {
  history: Record<string, SourceMessage[]> = {
    "c-general": [
      message("10", { content: "hi <:wave:400> <#302>", threadId: "10" }),
      message("11", { system: true, content: "" }),
      message("12", {
        authorName: "bob",
        at: T0 + DAY,
        content: "",
        pinned: true,
        attachments: [
          { name: "cat.png", url: "https://cdn/files/cat.png", size: 3, spoiler: false, width: 1, height: 1 },
          { name: "huge.zip", url: "https://cdn/files/huge.zip", size: 1 << 30, spoiler: false },
        ],
      }),
      message("13", { at: T0 + DAY, content: "x".repeat(4500) }),
    ],
    "302": [message("20", { content: "secret plans" })],
    // The thread started from message 10: Discord's starter message, then a reply.
    "10": [
      message("10", { system: true, content: "" }),
      message("15", {
        authorName: "bob",
        content: "in a thread",
        attachments: [{ name: "dog.png", url: "https://cdn/files/dog.png", size: 3, spoiler: false }],
      }),
    ],
    // A forum post: its first message has the post's id.
    "50": [
      message("50", { authorName: "carol", content: "I can't find the invite" }),
      message("51", {
        content: "use the link",
        reactions: [
          { emoji: "<:wave:400>", count: 2 },
          { emoji: "👍", count: 1 },
        ],
      }),
    ],
  };
  reads: string[] = [];

  async guild() {
    return guild;
  }

  async *messages(channelId: string, limit: number, after?: string) {
    const all = this.history[channelId] ?? [];
    const window = all.slice(Math.max(0, all.length - limit));
    for (const m of window) {
      if (after && BigInt(m.id) <= BigInt(after)) continue;
      this.reads.push(m.id);
      yield m;
    }
  }

  async forumPosts(channelId: string) {
    return channelId === "c-forum" ? [{ id: "50", name: "How do I join?" }] : [];
  }

  async download(url: string): Promise<Download> {
    return {
      bytes: new TextEncoder().encode(url),
      contentType: url.endsWith(".png") ? "image/png" : "application/zip",
    };
  }
}

class FakeTarget implements Target {
  serverId = "srv";
  calls: string[] = [];
  roles: { id: string; name: string; permissions: number[]; color?: number }[] = [];
  everyone: number[] = [];
  roleOrder: string[] = [];
  channels: {
    id: string;
    name: string;
    kind: string;
    parentId?: string;
    overwrites: TargetOverwrite[];
    topic: string;
  }[] = [];
  channelOrder: { id: string; parentId?: string }[] = [];
  emojis: { id: string; name: string }[] = [];
  posts: { hook: string; post: TargetPost; id: string }[] = [];
  files: { channelId: string; content: string; files: TargetFile[] }[] = [];
  replies: { channelId: string; threadId: string; content: string; files: string[] }[] = [];
  sounds: { name: string; emoji: string; volume: number }[] = [];
  pins: string[] = [];
  hooks = new Set<string>();
  lack: string[] = [];
  /** Throws once this many posts have gone through, as a stop would. */
  failAfterPosts = Infinity;
  #n = 0;
  #id = (kind: string) => `${kind}-${++this.#n}`;

  async missing() {
    return this.lack;
  }
  async updateServer(change: { name: string; description: string; icon?: Download }) {
    this.calls.push(`server ${change.name} ${change.description} ${change.icon ? "icon" : "no icon"}`);
  }
  async createRole(role: { name: string; color?: number; permissions: number[] }) {
    const id = this.#id("role");
    this.roles.push({ id, ...role });
    return id;
  }
  async setEveryone(permissions: number[]) {
    this.everyone = permissions;
  }
  async orderRoles(ids: string[]) {
    this.roleOrder = ids;
  }
  async createChannel(channel: {
    name: string;
    kind: string;
    parentId?: string;
    overwrites: TargetOverwrite[];
    topic: string;
  }) {
    const id = this.#id("chan");
    this.channels.push({ id, ...channel });
    return id;
  }
  async orderChannels(placed: { id: string; parentId?: string }[]) {
    this.channelOrder = placed;
  }
  async createEmoji(name: string) {
    const made = { id: this.#id("emoji"), name };
    this.emojis.push(made);
    return made;
  }
  async createHook(channelId: string): Promise<Hook> {
    const hook = { id: this.#id("hook"), channelId };
    this.hooks.add(hook.id);
    return hook;
  }
  async deleteHook(hook: Hook) {
    this.hooks.delete(hook.id);
  }
  /** How many posts each call through a webhook carried. */
  batches: number[] = [];
  /** Posts with this text are turned down, as AutoMod would. */
  refuse = "";
  async postMany(hook: Hook, posts: TargetPost[]) {
    // All of a batch or none of it, as fuwa does.
    if (this.posts.length + posts.length > this.failAfterPosts) throw new Error("the instance went away");
    this.batches.push(posts.length);
    return posts.map((post) => {
      if (this.refuse && post.content.includes(this.refuse)) return new Refused("AutoMod blocked it");
      const id = this.#id("msg");
      this.posts.push({ hook: hook.channelId, post, id });
      return id;
    });
  }
  async sendFiles(channelId: string, content: string, files: TargetFile[]) {
    this.files.push({ channelId, content, files });
    return this.#id("msg");
  }
  async reply(channelId: string, threadId: string, content: string, files: TargetFile[]) {
    this.replies.push({ channelId, threadId, content, files: files.map((f) => f.name) });
    return this.#id("msg");
  }
  hasSounds = true;
  async soundboard() {
    return this.hasSounds;
  }
  async createSound(sound: { name: string; emoji: string; volume: number }) {
    this.sounds.push({ name: sound.name, emoji: sound.emoji, volume: sound.volume });
  }
  async pin(_channelId: string, messageId: string) {
    this.pins.push(messageId);
  }
  async createInvite() {
    return "https://fuwa.example/invite/abc";
  }
}

const options = { messagesPerChannel: 100, channelsAtOnce: 1, maxFileBytes: 1 << 20 };

test("a whole server comes over", async () => {
  const source = new FakeSource();
  const target = new FakeTarget();
  const store = new MemoryStore();
  const report = await migrate(source, target, store, options);

  assert.deepEqual(target.calls, ["server Cozy Corner a nice place icon"]);

  // Roles: the bot's role stays behind, highest first in the order.
  assert.deepEqual(
    target.roles.map((r) => [r.name, r.color, r.permissions]),
    [
      ["Mod", 0x00ff00, [Permission.MANAGE_MESSAGES]],
      ["Admin", 0xff0000, [Permission.ADMINISTRATOR]],
    ],
  );
  assert.deepEqual(target.everyone, [Permission.VIEW_CHANNELS, Permission.SEND_MESSAGES, Permission.MANAGE_CARDS]);
  const roleId = (name: string) => target.roles.find((r) => r.name === name)!.id;
  assert.deepEqual(target.roleOrder, [roleId("Admin"), roleId("Mod")]);

  // Channels: the category first, a forum as text, the directory left out.
  const chan = (name: string) => target.channels.find((c) => c.name === name)!;
  assert.deepEqual(
    target.channels.map((c) => [c.name, c.kind]),
    [
      ["Chat", "category"],
      ["help", "text"],
      ["general", "text"],
      ["mods", "text"],
      ["Lounge", "voice"],
    ],
  );
  assert.equal(chan("general").parentId, chan("Chat").id);
  assert.deepEqual(chan("mods").overwrites, [
    { targetId: "srv", kind: "role", allow: [], deny: [Permission.VIEW_CHANNELS] },
    { targetId: roleId("Mod"), kind: "role", allow: [Permission.VIEW_CHANNELS], deny: [] },
  ]);
  assert.deepEqual(
    target.channelOrder.map((p) => target.channels.find((c) => c.id === p.id)!.name),
    ["help", "Chat", "general", "mods", "Lounge"],
  );

  // Emoji, and the sticker fuwa can show.
  assert.deepEqual(
    target.emojis.map((e) => e.name),
    ["wave", "cat"],
  );

  // Sounds, with their emoji, names that don't clash.
  assert.deepEqual(target.sounds, [
    { name: "airhorn", emoji: `<:wave:${target.emojis[0]!.id}>`, volume: 0.5 },
    { name: "Airhorn 2", emoji: "📯", volume: 1 },
  ]);

  // History: a divider per day, authors' names and pictures, links rewritten.
  const general = target.posts.filter((p) => p.hook === chan("general").id).map((p) => p.post);
  const wave = target.emojis.find((e) => e.name === "wave")!;
  assert.deepEqual(
    general.map((p) => [p.username, p.content.slice(0, 40)]),
    [
      [IMPORTER_NAME, "── <t:1704110400:D> ──"],
      ["alice", `hi <:wave:${wave.id}> <#${chan("mods").id}>`],
      [IMPORTER_NAME, "── <t:1704196800:D> ──"],
      ["bob", "*(a file not copied: huge.zip)*"],
      ["alice", "x".repeat(40)],
      ["alice", "x".repeat(40)],
    ],
  );
  assert.equal(general[1]!.avatarUrl, "https://cdn.discordapp.com/avatars/1/a.png");
  // The files follow from the agent, and the pin lands on bob's first message.
  assert.deepEqual(
    target.files.map((f) => [f.content, f.files.map((x) => x.name)]),
    [["Files from **bob**", ["cat.png"]]],
  );
  assert.equal(target.pins.length, 1);
  assert.equal(target.hooks.size, 0, "the webhooks are deleted afterwards");

  // The thread under alice's message, its reply from the agent with bob's name and file.
  const anchor = target.posts.find((p) => p.post.content.startsWith("hi "))!.id;
  assert.deepEqual(
    target.replies.filter((r) => r.channelId === chan("general").id),
    [
      {
        channelId: chan("general").id,
        threadId: anchor,
        content: `**bob** · <t:${T0 / 1000}:f>\nin a thread`,
        files: ["dog.png"],
      },
    ],
  );

  // The forum post: its first message through the webhook with the title, the rest in its thread.
  const help = target.posts.filter((p) => p.hook === chan("help").id).map((p) => p.post);
  assert.deepEqual(
    help.map((p) => [p.username, p.content]),
    [["carol", "**How do I join?**\nI can't find the invite"]],
  );
  assert.equal(target.replies[0]!.threadId, target.posts.find((p) => p.hook === chan("help").id)!.id);
  assert.equal(
    target.replies[0]!.content,
    `**alice** · <t:${T0 / 1000}:f>\nuse the link\n*Reactions: <:wave:${wave.id}> 2 · 👍 1*`,
  );

  assert.equal(report.phase, "done");
  assert.equal(report.messages, 7);
  assert.equal(report.threads, 2);
  assert.equal(report.files, 2);
  assert.equal(report.pins, 1);
  assert.equal(report.historyDone, 3);
  assert.equal(report.sounds, 2);
  assert.equal(report.invite, "https://fuwa.example/invite/abc");
  assert.ok(report.skipped.some((s) => s.includes("dance")));
  assert.ok(report.skipped.some((s) => s.includes("#directory")));

  const summary = summaryText(report);
  assert.match(
    summary,
    /2 roles, 5 channels, 1 emoji, 1 sticker, 2 sounds, 7 messages \(with 2 threads\), 2 files and 1 pin/,
  );
  assert.match(summary, /https:\/\/fuwa\.example\/invite\/abc/);
  assert.match(progressText(report, true), /✅ 2 sounds/);
  assert.match(progressText(report, true), /✅ History: 3 of 3 channels, 7 messages, 2 threads, 2 files/);
});

test("history goes out in batches, and one message turned down leaves out just that one", async () => {
  const target = new FakeTarget();
  target.refuse = "secret";
  const report = await migrate(new FakeSource(), target, new MemoryStore(), options);
  // The first day's divider and message, the second day's divider and bob's text go out together, before bob's files.
  assert.ok(target.batches.includes(4), `batches: ${target.batches}`);
  assert.ok(target.batches.length < target.posts.length);
  assert.ok(!target.posts.some((p) => p.post.content.includes("secret")));
  assert.ok(target.posts.some((p) => p.post.content.startsWith("xxx")));
  assert.ok(report.skipped.some((s) => s.startsWith("A message by") && s.includes("AutoMod")));
});

test("only what was chosen comes over, without the channels left out", async () => {
  const target = new FakeTarget();
  const report = await migrate(new FakeSource(), target, new MemoryStore(), {
    ...options,
    parts: ["roles", "channels", "history"],
    leaveOut: ["c-cat"],
  });
  assert.deepEqual(target.calls, []);
  assert.deepEqual(target.emojis, []);
  assert.deepEqual(target.sounds, []);
  assert.equal(report.invite, undefined);
  assert.equal(target.roles.length, 2);
  // The category, and the channels in it, stay behind; the directory is still one fuwa can't have.
  assert.deepEqual(
    target.channels.map((c) => c.name),
    ["help"],
  );
  assert.ok(report.skipped.some((s) => s.includes("#directory")));
  assert.ok(!report.skipped.some((s) => s.includes("#general")));
  assert.equal(report.historyTotal, 1);
  assert.ok(target.posts.every((p) => p.hook === target.channels[0]!.id));
  const progress = progressText(report, true);
  assert.doesNotMatch(progress, /emoji|sound|invite|icon/i);
  assert.match(progress, /✅ 2 roles/);
  const summary = summaryText(report);
  assert.match(summary, /2 roles, 1 channel, 2 messages \(with 1 thread\), 0 files and 0 pins\./);
  assert.doesNotMatch(summary, /emoji|sound|Invite your/);

  // Picking up later with the channels left out before copies just those.
  const store = new MemoryStore();
  const later = new FakeTarget();
  await migrate(new FakeSource(), later, store, { ...options, parts: ["channels"], leaveOut: ["c-cat"] });
  await migrate(new FakeSource(), later, store, { ...options, parts: ["channels"] });
  assert.deepEqual(
    later.channels.map((c) => c.name),
    ["help", "Chat", "general", "mods", "Lounge"],
  );
});

test("a stopped run picks up where it left off, copying nothing twice", async () => {
  const source = new FakeSource();
  const store = new MemoryStore();
  const first = new FakeTarget();
  first.failAfterPosts = 5;
  await assert.rejects(migrate(source, first, store, options), /went away/);
  assert.equal(first.hooks.size, 0);

  // Same server on fuwa, so the same state: nothing but the rest of the history is made.
  const second = new FakeTarget();
  const report = await migrate(source, second, store, options, newReport());
  assert.deepEqual(second.calls, []);
  assert.deepEqual(second.roles, []);
  assert.deepEqual(second.channels, []);
  assert.deepEqual(second.emojis, []);
  const posted = [...first.posts, ...second.posts].filter((p) => p.post.username !== IMPORTER_NAME);
  assert.deepEqual(
    posted.map((p) => p.post.content.slice(0, 12)),
    ["**How do I j", "hi <:wave:em", "*(a file not", "xxxxxxxxxxxx", "xxxxxxxxxxxx", "secret plans"],
  );
  // The thread under message 10 still comes over, under the copy made before the stop.
  assert.deepEqual(
    [...first.replies, ...second.replies].map((r) => r.content.split("\n")[1]),
    ["use the link", "in a thread"],
  );
  assert.equal(second.replies[0]!.threadId, first.posts.find((p) => p.post.content.startsWith("hi "))!.id);
  // The second day's divider went out before the stop, so it isn't posted again.
  const general = second.channels.length ? [] : second.posts.filter((p) => p.hook === first.channels[2]!.id);
  assert.deepEqual(
    general.filter((p) => p.post.username === IMPORTER_NAME),
    [],
  );
  assert.equal(report.messages, 7);
});

test("on a fuwa without a soundboard, the rest still comes over", async () => {
  const target = new FakeTarget();
  target.hasSounds = false;
  const withExpressions = {
    ...guild,
    roles: guild.roles.map((r) =>
      r.id === "r-mod" ? { ...r, permissions: r.permissions | (1n << 30n) | (1n << 42n) } : r,
    ),
  };
  const source = new FakeSource();
  source.guild = async () => withExpressions;
  const report = await migrate(source, target, new MemoryStore(), { ...options, messagesPerChannel: 0 });
  assert.deepEqual(target.sounds, []);
  assert.deepEqual(target.roles.find((r) => r.name === "Mod")!.permissions, [
    Permission.MANAGE_MESSAGES,
    Permission.MANAGE_EMOJI,
  ]);
  assert.ok(report.skipped.includes("2 sounds (this fuwa has no soundboard yet)"));
});

test("history can be left out, and nothing starts without the permissions", async () => {
  const target = new FakeTarget();
  const report = await migrate(new FakeSource(), target, new MemoryStore(), { ...options, messagesPerChannel: 0 });
  assert.equal(target.posts.length, 0);
  assert.equal(report.historyTotal, 0);
  assert.doesNotMatch(progressText(report, false), /History/);

  const blocked = new FakeTarget();
  blocked.lack = ["Give @import Administrator"];
  await assert.rejects(migrate(new FakeSource(), blocked, new MemoryStore(), options), NotReadyError);
  assert.deepEqual(blocked.roles, []);
});

test("cancelling stops between messages", async () => {
  const controller = new AbortController();
  const target = new FakeTarget();
  const source = new FakeSource();
  let seen = 0;
  const run = migrate(source, target, new MemoryStore(), {
    ...options,
    signal: controller.signal,
    onProgress: (r) => {
      if (r.messages > seen) {
        seen = r.messages;
        controller.abort(new Error("it was cancelled"));
      }
    },
  });
  await assert.rejects(run, /cancelled/);
  assert.equal(seen, 1);
  assert.equal(target.hooks.size, 0);
});
