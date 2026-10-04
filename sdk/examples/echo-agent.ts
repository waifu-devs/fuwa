// A small agent: /echo, /dice, /help, and a hello when someone mentions it.
//
//   FUWA_URL=https://fuwa.chat FUWA_TOKEN=... node examples/echo-agent.ts
//
// (Node 22.6+ runs TypeScript directly; with Node 20, build it with tsc first.)
// Make the agent in Settings > Agents, copy its token, then add it to a server
// from Server settings > Integrations. The token is a secret: keep it in the
// environment or a secret store, never in code, and reset it if it leaks.
import { readFileSync, writeFileSync } from "node:fs";
import { Agent, RateLimitedError } from "@waifu-devs/fuwa";

const url = process.env.FUWA_URL ?? "http://localhost:8080";
const token = process.env.FUWA_TOKEN;
if (!token) {
  console.error("Set FUWA_TOKEN to the agent's token.");
  process.exit(1);
}

// Where the agent left off, so a restart answers what was said while it was down.
const CURSORS = process.env.FUWA_CURSORS ?? ".fuwa-cursors.json";
function loadCursors(): Record<string, bigint> {
  try {
    const saved = JSON.parse(readFileSync(CURSORS, "utf8")) as Record<string, string>;
    return Object.fromEntries(Object.entries(saved).map(([id, seq]) => [id, BigInt(seq)]));
  } catch {
    return {};
  }
}
function saveCursors(agent: Agent) {
  const out = Object.fromEntries([...agent.cursors].map(([id, seq]) => [id, seq.toString()]));
  writeFileSync(CURSORS, JSON.stringify(out));
}

const agent = new Agent({ url, token, cursors: loadCursors() });

agent.command("echo", (ctx) => ctx.reply(ctx.rest || "Say something after /echo."), {
  description: "Says it back",
});

agent.command(
  "dice",
  (ctx) => {
    // "2d6+1", "d20", or nothing for one six-sided die.
    const m = /^(\d{0,2})d(\d{1,4})([+-]\d{1,4})?$/i.exec(ctx.args[0] ?? "d6");
    if (!m) return ctx.reply("Try `/dice 2d6+1`.");
    const count = Math.min(Math.max(Number(m[1] || 1), 1), 20);
    const sides = Math.max(Number(m[2]), 2);
    const bonus = Number(m[3] ?? 0);
    const rolls = Array.from({ length: count }, () => 1 + Math.floor(Math.random() * sides));
    const total = rolls.reduce((a, b) => a + b, 0) + bonus;
    return ctx.reply(`🎲 ${rolls.join(" + ")}${bonus ? ` ${m[3]}` : ""} = **${total}**`);
  },
  { description: "Rolls dice, like 2d6+1", aliases: ["roll"] },
);

agent.command(
  "help",
  (ctx) => ctx.reply(agent.commands.map((c) => `\`${agent.prefix}${c.name}\` ${c.description}`).join("\n")),
  { description: "Lists what I can do" },
);

agent.on("mention", async (ctx) => {
  if (ctx.content.trim().toLowerCase().startsWith(`@${agent.me.username}`)) return; // a command
  const author = await ctx.author();
  await ctx.reply(`Hi ${author?.displayName || "there"}! Try \`${agent.prefix}help\`.`);
});

agent.on("ready", ({ me, servers }) => console.log(`Signed in as @${me.username}, in ${servers.length} server(s).`));
agent.on("serverAdded", () => console.log("Added to a server."));
agent.on("disconnected", ({ error, retryInMs }) =>
  console.log(`Lost the connection (${error.message}); trying again in ${Math.round(retryInMs / 1000)}s.`),
);
agent.on("error", (err) => {
  if (err instanceof RateLimitedError) console.log(`Slowed down: ${err.message}`);
  else console.error(err instanceof Error ? `${err.name}: ${err.message}` : err);
});

// Save where it is every few seconds, and on the way out.
const saver = setInterval(() => saveCursors(agent), 5000);
for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, async () => {
    clearInterval(saver);
    await agent.stop();
    saveCursors(agent);
    process.exit(0);
  });
}

await agent.start();
await agent.closed;
