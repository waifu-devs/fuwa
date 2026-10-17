// Benchmarks the web client on one channel: opens the app in Chromium as one
// person, floods the channel from another at a given rate, and prints what
// the page spent to keep up (main-thread CPU, frames, long tasks, DOM).
//
//   ./start.sh                     # the seeded instance (README.md)
//   node client-load.mjs [--rate 50] [--secs 20] [--as alice] [--flood bob]
//
// docs/capacity.md has what it found. Needs the SDK built (README's once
// steps) and Playwright's Chromium; the numbers are for the machine it runs
// on, not for a phone or a desktop with a GPU.
import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { createFuwa } from "../../../sdk/dist/esm/index.js";

const require = createRequire(import.meta.url);
let pw;
try {
  pw = require("playwright");
} catch {
  pw = require(`${execSync("npm root -g").toString().trim()}/playwright`);
}
const { chromium } = pw;

const vis = process.env.FUWA_VISUAL_DIR ?? "/tmp/fuwa-visual";
const st = JSON.parse(readFileSync(`${vis}/state.json`, "utf8"));
const args = process.argv.slice(2);
const opt = (k, d) => {
  const i = args.indexOf(k);
  return i >= 0 ? args[i + 1] : d;
};
const as = opt("--as", "alice");
const floodAs = opt("--flood", "bob");
const rate = Number(opt("--rate", 50));
const secs = Number(opt("--secs", 20));
/// Where the web app is; the instance itself when it serves it (the usual
/// case), or a dev server on the instance's allowed origins.
const app = opt("--app", st.url);

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1, colorScheme: "dark" });
const saved = [
  {
    url: st.url,
    active: st.ids[as],
    accounts: [{ userId: st.ids[as], token: st.tokens[as], username: as, displayName: as, avatarUrl: "" }],
  },
];
await ctx.addInitScript((v) => localStorage.setItem("fuwa:accounts:v2", v), JSON.stringify(saved));
await ctx.addInitScript(() => {
  window.__longTasks = [];
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) window.__longTasks.push(entry.duration);
  }).observe({ type: "longtask", buffered: true });
  window.__frames = 0;
  const tick = () => {
    window.__frames++;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
});

const page = await ctx.newPage();
page.on("crash", () => console.error("the page crashed"));
const cdp = await ctx.newCDPSession(page);
await cdp.send("Performance.enable");
const route = `${app}/${encodeURIComponent(st.host)}/${st.server}/${st.channels.general}`;
await page.goto(route);
await page.waitForTimeout(4000); // sign in, load state, open the channel
const metrics = async () => {
  const { metrics } = await cdp.send("Performance.getMetrics");
  return Object.fromEntries(metrics.map((m) => [m.name, m.value]));
};
const before = await metrics();

// The flood, from its own client so the page's event loop is not shared.
const fuwa = createFuwa({ url: st.url, token: st.tokens[floodAs], retry: false });
let sent = 0,
  failed = 0,
  inflight = 0,
  carry = 0,
  last = Date.now();
const started = Date.now();
const tick = setInterval(() => {
  const now = Date.now();
  carry += (rate * (now - last)) / 1000;
  last = now;
  let n = Math.floor(carry);
  carry -= n;
  while (n-- > 0 && inflight < 32) {
    sent++;
    inflight++;
    fuwa.messages
      .sendMessage({ serverId: st.server, channelId: st.channels.general, content: `flood ${sent} a line about the length people type` })
      .catch(() => failed++)
      .finally(() => inflight--);
  }
}, 20);
await page.waitForTimeout(secs * 1000);
clearInterval(tick);
const drained = Date.now() + 5000;
while (inflight > 0 && Date.now() < drained) await new Promise((r) => setTimeout(r, 20));

const after = await metrics();
const dom = await page.evaluate(() => ({
  messages: document.querySelectorAll("[data-message-id]").length,
  nodes: document.getElementsByTagName("*").length,
  longTasks: window.__longTasks.slice(),
  frames: window.__frames,
}));
const seconds = (Date.now() - started) / 1000;
console.log(
  JSON.stringify(
    {
      rate_target: rate,
      secs,
      sent,
      failed,
      task_cpu_secs: after.TaskDuration - before.TaskDuration,
      script_secs: after.ScriptDuration - before.ScriptDuration,
      layout_secs: after.LayoutDuration - before.LayoutDuration,
      frames_per_sec: dom.frames / seconds,
      dom_messages: dom.messages,
      dom_nodes: dom.nodes,
      long_tasks: dom.longTasks.length,
      long_task_ms: dom.longTasks.reduce((a, b) => a + b, 0),
      longest_task_ms: dom.longTasks.length ? Math.max(...dom.longTasks) : 0,
      js_heap_mb: after.JSHeapUsedSize / 1048576,
      cpu_ms_per_message: (1000 * (after.TaskDuration - before.TaskDuration)) / Math.max(1, sent),
    },
    null,
    2,
  ),
);
await browser.close();
