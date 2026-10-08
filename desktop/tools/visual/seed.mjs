// Seeds the visual-comparison instance: accounts, three servers, channels, roles, messages.
// The SDK from this checkout: build it first (cd sdk && pnpm install && pnpm build).
import { createFuwa } from "../../../sdk/dist/esm/index.js";
import { mkdirSync, writeFileSync } from "node:fs";
const url = `http://127.0.0.1:${process.env.FUWA_VISUAL_PORT ?? 18080}`;
const vis = process.env.FUWA_VISUAL_DIR ?? "/tmp/fuwa-visual";
const anon = createFuwa({ url });
const people = [
  ["alice", "Alice"], ["bob", "Bob"], ["carol", "Carol"], ["dave", "Dave"], ["erin", "Erin ✿"], ["frank", "Frank"],
];
const tok = {}, ids = {};
for (const [u, d] of people) {
  const r = await anon.auth.signUp({ username: u, password: "password123!", displayName: d });
  tok[u] = r.token; ids[u] = r.user.id;
}
const c = (u) => createFuwa({ url, token: tok[u] });
const A = c("alice");
const T = { TEXT: 1, VOICE: 2, ANN: 3, CAT: 5 };
const mk = async (name, desc) => (await A.servers.createServer({ name, description: desc, discoverable: true })).server;
const s = await mk("Waifu Devs", "Building fuwa together");
const s2 = await mk("Art Club", "Drawings and doodles");
const s3 = await mk("Gaming", "Game nights");
const ch = async (srv, name, type, parentId = "", topic = "") =>
  (await A.channels.createChannel({ serverId: srv.id, name, type, parentId, topic })).channel;
const existing = (await A.channels.listChannels({ serverId: s.id })).channels;
const text = await ch(s, "Text Channels", T.CAT);
const voice = await ch(s, "Voice", T.CAT);
let general = existing.find((x) => x.name === "general");
if (general) await A.channels.updateChannel({ serverId: s.id, channelId: general.id, name: "general", topic: "Say hi! Be kind ✿", parentId: text.id }).catch(() => {});
else general = await ch(s, "general", T.TEXT, text.id, "Say hi! Be kind ✿");
const ann = await ch(s, "announcements", T.ANN, text.id, "News about fuwa");
const random = await ch(s, "random", T.TEXT, text.id);
const art = await ch(s, "art", T.TEXT, text.id);
const lounge = await ch(s, "Lounge", T.VOICE, voice.id);
const inv = (await A.invites.createInvite({ serverId: s.id })).invite;
for (const [u] of people.slice(1)) {
  await c(u).servers.joinServer({ serverId: s.id, inviteCode: inv.code });
}
for (const srv of [s2, s3]) {
  const i = (await A.invites.createInvite({ serverId: srv.id })).invite;
  await c("bob").servers.joinServer({ serverId: srv.id, inviteCode: i.code });
}
const role = async (name, color, hoist) => (await A.roles.createRole({ serverId: s.id, name, color, hoist, mentionable: true })).role;
const admin = await role("Admin", 0xe5484d, true);
const mod = await role("Moderator", 0x3b82f6, true);
const member = await role("Member", 0x22c55e, false);
const give = (u, r) => A.roles.addMemberRole({ serverId: s.id, userId: ids[u], roleId: r.id });
await give("alice", admin); await give("bob", mod); await give("carol", mod);
for (const u of ["dave", "erin", "frank"]) await give(u, member);
const say = async (u, content, extra = {}) =>
  (await c(u).messages.sendMessage({ serverId: s.id, channelId: general.id, content, ...extra })).message;
const m1 = await say("alice", "Welcome to **Waifu Devs**! This is where we build fuwa together ✿");
await say("bob", "hi everyone! 👋");
await say("bob", "just pulled master, the new rail folders are *so* nice");
await say("carol", "Has anyone tried the desktop app on Linux yet?");
const q = await say("dave", "yes! it runs great on Wayland. had to install `libxkbcommon` first though");
await say("carol", "thanks, that did it", { replyToId: q.id });
await say("erin", "> the desktop app should look exactly like the web\nagreed, pixel for pixel 🎨");
await say("alice", `Quick reminder for <@&${mod.id}>: the release is on Friday. <@${ids.bob}> can you write the notes?`);
await say("bob", "on it ✍️");
await say("frank", "Here's the snippet I used to test the API:\n```ts\nconst fuwa = createFuwa({ url, token });\nconst me = await fuwa.auth.getMe({});\nconsole.log(me.user?.displayName);\n```");
await say("dave", "Docs are at https://github.com/waifu-devs/fuwa if anyone needs them");
const ed = await say("carol", "I'll review the PR tonight");
await c("carol").messages.updateMessage({ serverId: s.id, channelId: general.id, messageId: ed.id, content: "I'll review the PR tonight, after dinner" }).catch(() => {});
await say("erin", "Can we get a ~~dark~~ darker theme? 🌙");
await say("alice", "There are five built-in themes already, check Settings → Appearance");
await say("bob", "Yoru is my favorite");
await say("frank", "Sakura for life 🌸");
await say("alice", "Pinned the roadmap above, have a look when you get a chance!");
await A.messages.pinMessage({ serverId: s.id, channelId: general.id, messageId: m1.id, pinned: true }).catch((e) => console.error("pin", e.message));
await say("dave", "Poll time!", { poll: { question: "Which theme do you use?", answers: [{ text: "Sakura" }, { text: "Yoru" }, { text: "Something custom" }] } }).catch((e) => console.error("poll", e.message));
const t = await say("carol", "Thread for release notes ideas");
await c("bob").messages.sendMessage({ serverId: s.id, channelId: general.id, threadId: t.id, content: "mention the GIF picker!" }).catch((e) => console.error("thread", e.message));
await c("erin").messages.sendMessage({ serverId: s.id, channelId: random.id, content: "random thoughts go here" });
await A.messages.sendMessage({ serverId: s.id, channelId: ann.id, content: "fuwa 0.3 is out! 🎉" });
const state = { url, host: new URL(url).host, ids, tokens: tok, server: s.id, servers: [s.id, s2.id, s3.id], channels: { general: general.id, announcements: ann.id, random: random.id, art: art.id, lounge: lounge.id } };
mkdirSync(vis, { recursive: true });
writeFileSync(`${vis}/state.json`, JSON.stringify(state, null, 2));
console.log("seeded", s.id, general.id);
