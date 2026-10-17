// The fuwa side against a real instance ($FUWA_BIN, built with
// `cargo build -p fuwa-server --bin fuwa`): a made-up Discord server copied
// into a fuwa server through the agent and webhooks, then read back as a
// member sees it. Skipped without FUWA_BIN; run with `pnpm test:instance`.
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { Agent, ChannelType, type Fuwa, Permission, createFuwa, messages } from "@waifu-devs/fuwa";
import { FuwaTarget } from "../src/fuwa.ts";
import { IMPORTER_NAME, migrate } from "../src/migrate.ts";
import type { Download, Source, SourceGuild, SourceMessage } from "../src/model.ts";
import { MemoryStore } from "../src/state.ts";

const bin = process.env.FUWA_BIN;
const skip = bin ? false : "set FUWA_BIN to a fuwa binary";

// The smallest valid PNG: one transparent pixel.
const PIXEL = Uint8Array.from(
  atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="),
  (c) => c.charCodeAt(0),
);

let child: ChildProcess | undefined;
let data = "";
let url = "";
let person: Fuwa;
let serverId = "";
let agent: Agent;

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = createServer();
    srv.on("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const { port } = srv.address() as { port: number };
      srv.close(() => resolve(port));
    });
  });
}

before(async () => {
  if (!bin) return;
  const port = await freePort();
  data = mkdtempSync(join(tmpdir(), "discord-migrate-"));
  url = `http://127.0.0.1:${port}`;
  child = spawn(bin, [], {
    env: {
      ...process.env,
      FUWA_DATA_PATH: data,
      FUWA_HOST: "127.0.0.1",
      FUWA_PORT: String(port),
      FUWA_PUBLIC_URL: url,
      FUWA_TELEMETRY: "off",
      FUWA_WEB: "off",
      RUST_LOG: "warn",
    },
    stdio: "ignore",
  });
  for (let i = 0; ; i++) {
    try {
      if ((await fetch(`${url}/healthz`)).ok) break;
    } catch {
      // not listening yet
    }
    if (i > 300) throw new Error("fuwa didn't start");
    await new Promise((r) => setTimeout(r, 100));
  }
  const { token } = await createFuwa({ url }).auth.signUp({ username: "owner", password: "correct horse battery" });
  person = createFuwa({ url, token });
  serverId = (await person.servers.createServer({ name: "New home" })).server!.id;
  const made = await person.agents.createAgent({ username: "migrate", displayName: "Migrate" });
  await person.agents.addAgent({ serverId, username: "migrate" });
  agent = new Agent({ url, token: made.token, onError: (e) => assert.fail(e as Error) });
  await agent.start();
});

after(async () => {
  await agent?.stop();
  child?.kill("SIGTERM");
  if (data) rmSync(data, { recursive: true, force: true });
});

const guild: SourceGuild = {
  id: "100",
  name: "Cozy Corner",
  description: "Where cozy people hang out",
  iconUrl: "https://cdn.example/icon.png",
  roles: [
    {
      id: "100",
      name: "@everyone",
      color: 0,
      permissions: (1n << 10n) | (1n << 11n) | (1n << 6n),
      hoist: false,
      mentionable: false,
      position: 0,
      managed: false,
      everyone: true,
    },
    {
      id: "200",
      name: "Mods",
      color: 0x3498db,
      permissions: (1n << 13n) | (1n << 1n),
      hoist: true,
      mentionable: true,
      position: 1,
      managed: false,
      everyone: false,
    },
  ],
  channels: [
    { id: "300", name: "Text Channels", kind: "category", position: 0, topic: "", slowmodeSeconds: 0, overwrites: [] },
    {
      id: "301",
      name: "general",
      kind: "text",
      parentId: "300",
      position: 0,
      topic: "Say hi",
      slowmodeSeconds: 0,
      overwrites: [],
    },
    {
      id: "302",
      name: "mod-chat",
      kind: "text",
      parentId: "300",
      position: 1,
      topic: "",
      slowmodeSeconds: 0,
      overwrites: [
        { id: "100", kind: "role", allow: 0n, deny: 1n << 10n },
        { id: "200", kind: "role", allow: 1n << 10n, deny: 0n },
      ],
    },
    {
      id: "303",
      name: "Lounge",
      kind: "voice",
      parentId: "300",
      position: 0,
      topic: "",
      slowmodeSeconds: 0,
      overwrites: [],
    },
  ],
  emojis: [{ id: "400", name: "blob-wave", url: "https://cdn.example/emoji.png", animated: false }],
  stickers: [],
};

const at = Date.UTC(2023, 4, 1, 9);
const history: Record<string, SourceMessage[]> = {
  "301": [
    {
      id: "1000",
      authorName: "alice",
      at,
      content: "hello <:blob_wave:400> see <#302>",
      system: false,
      pinned: true,
      attachments: [],
      embeds: [],
      stickers: [],
      mentionedUsers: {},
    },
    {
      id: "1001",
      authorName: "bob",
      at: at + 60_000,
      content: "a picture",
      system: false,
      pinned: false,
      replyTo: { authorName: "alice", content: "hello" },
      attachments: [
        {
          name: "pixel.png",
          url: "https://cdn.example/pixel.png",
          size: PIXEL.length,
          spoiler: false,
          width: 1,
          height: 1,
        },
      ],
      embeds: [{ title: "A card", description: "from a bot", fields: [] }],
      stickers: [],
      mentionedUsers: {},
    },
  ],
};

const source: Source = {
  guild: async () => guild,
  async *messages(channelId, limit, after) {
    for (const m of (history[channelId] ?? []).slice(-limit)) if (!after || BigInt(m.id) > BigInt(after)) yield m;
  },
  download: async (): Promise<Download> => ({ bytes: PIXEL, contentType: "image/png" }),
};

test("without Administrator, the agent says what it needs", { skip }, async () => {
  const missing = await new FuwaTarget(agent, serverId).missing();
  assert.match(missing.join(), /@migrate a role with Administrator/);
});

test("a Discord server comes over, as members see it", { skip }, async () => {
  const { role } = await person.roles.createRole({ serverId, name: "Bots", permissions: [Permission.ADMINISTRATOR] });
  await person.roles.addMemberRole({ serverId, roleId: role!.id, userId: agent.me.id });

  const target = new FuwaTarget(agent, serverId, () => Promise.resolve());
  const report = await migrate(source, target, new MemoryStore(), {
    messagesPerChannel: 100,
    channelsAtOnce: 2,
    maxFileBytes: 1 << 20,
  });
  assert.deepEqual(report.skipped, []);

  const { server } = await person.servers.getServer({ serverId });
  assert.equal(server!.name, "Cozy Corner");
  assert.equal(server!.description, "Where cozy people hang out");
  assert.ok(server!.iconUrl);

  const { roles } = await person.roles.listRoles({ serverId });
  const mods = roles.find((r) => r.name === "Mods")!;
  assert.equal(mods.color, 0x3498db);
  assert.ok(mods.hoist && mods.mentionable);
  assert.deepEqual(mods.permissions, [Permission.KICK_MEMBERS, Permission.MANAGE_MESSAGES]);
  const everyone = roles.find((r) => r.id === serverId)!;
  assert.deepEqual(everyone.permissions, [
    Permission.VIEW_CHANNELS,
    Permission.SEND_MESSAGES,
    Permission.ADD_REACTIONS,
    Permission.MANAGE_CARDS,
  ]);

  const { channels } = await person.channels.listChannels({ serverId });
  const named = (name: string) => channels.find((c) => c.name === name)!;
  const category = named("Text Channels");
  assert.equal(category.type, ChannelType.CATEGORY);
  assert.equal(named("general").parentId, category.id);
  assert.equal(named("general").topic, "Say hi");
  assert.equal(named("Lounge").type, ChannelType.VOICE);
  // Discord's order comes first; the server's own channels follow.
  assert.deepEqual(
    channels.slice(0, 4).map((c) => c.name),
    ["Text Channels", "general", "mod-chat", "Lounge"],
  );
  const modChat = named("mod-chat");
  const hidden = modChat.permissionOverwrites.find((o) => o.targetId === serverId)!;
  assert.deepEqual(hidden.deny, [Permission.VIEW_CHANNELS]);
  assert.deepEqual(modChat.permissionOverwrites.find((o) => o.targetId === mods.id)!.allow, [Permission.VIEW_CHANNELS]);

  const { emojis } = await person.emojis.listEmojis({ serverId });
  assert.deepEqual(
    emojis.map((e) => e.name),
    ["blob_wave"],
  );

  // History, oldest first: the day's divider, alice's message, bob's reply and his files.
  const general = [];
  for await (const { message } of messages(person, { serverId, channelId: named("general").id }))
    general.unshift(message);
  assert.deepEqual(
    general.map((m) => m.webhook?.name ?? `agent: ${m.content}`),
    [IMPORTER_NAME, "alice", "bob", "agent: Files from **bob**"],
  );
  assert.equal(general[0]!.content, `── <t:${at / 1000}:D> ──`);
  assert.equal(general[1]!.content, `hello <:blob_wave:${emojis[0]!.id}> see <#${modChat.id}>`);
  assert.equal(general[2]!.content, "> ↪ **alice**: hello\na picture");
  assert.equal(general[2]!.embeds[0]!.title, "A card");
  assert.equal(general[3]!.attachments[0]!.filename, "pixel.png");
  const { messages: pins } = await person.messages.listPins({ serverId, channelId: named("general").id });
  assert.deepEqual(
    pins.map((m) => m.id),
    [general[1]!.id],
  );

  // The webhooks are gone, and the invite lets people in.
  assert.deepEqual((await person.webhooks.listWebhooks({ serverId })).webhooks, []);
  const code = report.invite!.split("/invite/")[1]!;
  const { invite } = await person.invites.getInvite({ code });
  assert.equal(invite!.serverId, serverId);
});
