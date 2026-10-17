import assert from "node:assert/strict";
import { test } from "node:test";
import { Permission } from "@waifu-devs/fuwa";
import { IMPORTER_NAME, NotReadyError, migrate, newReport } from "../src/migrate.ts";
import type {
  Download,
  Hook,
  Source,
  SourceGuild,
  SourceMessage,
  Target,
  TargetFile,
  TargetOverwrite,
  TargetPost,
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
};

class FakeSource implements Source {
  history: Record<string, SourceMessage[]> = {
    "c-general": [
      message("10", { content: "hi <:wave:400> <#302>" }),
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
  posts: { hook: string; post: TargetPost }[] = [];
  files: { channelId: string; content: string; files: TargetFile[] }[] = [];
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
  async post(hook: Hook, post: TargetPost) {
    if (this.posts.length >= this.failAfterPosts) throw new Error("the instance went away");
    this.posts.push({ hook: hook.channelId, post });
    return this.#id("msg");
  }
  async sendFiles(channelId: string, content: string, files: TargetFile[]) {
    this.files.push({ channelId, content, files });
    return this.#id("msg");
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

  assert.equal(report.phase, "done");
  assert.equal(report.messages, 4);
  assert.equal(report.files, 1);
  assert.equal(report.pins, 1);
  assert.equal(report.historyDone, 2);
  assert.equal(report.invite, "https://fuwa.example/invite/abc");
  assert.ok(report.skipped.some((s) => s.includes("dance")));
  assert.ok(report.skipped.some((s) => s.includes("#help's posts")));
  assert.ok(report.skipped.some((s) => s.includes("#directory")));

  const summary = summaryText(report);
  assert.match(summary, /2 roles, 5 channels, 1 emoji, 1 sticker, 4 messages, 1 file and 1 pin/);
  assert.match(summary, /https:\/\/fuwa\.example\/invite\/abc/);
  assert.match(progressText(report, true), /✅ History: 2 of 2 channels, 4 messages, 1 file/);
});

test("a stopped run picks up where it left off, copying nothing twice", async () => {
  const source = new FakeSource();
  const store = new MemoryStore();
  const first = new FakeTarget();
  first.failAfterPosts = 3;
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
    ["hi <:wave:em", "*(a file not", "xxxxxxxxxxxx", "xxxxxxxxxxxx", "secret plans"],
  );
  // The second day's divider went out before the stop, so it isn't posted again.
  const general = second.channels.length ? [] : second.posts.filter((p) => p.hook === first.channels[2]!.id);
  assert.deepEqual(
    general.filter((p) => p.post.username === IMPORTER_NAME),
    [],
  );
  assert.equal(report.messages, 4);
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
