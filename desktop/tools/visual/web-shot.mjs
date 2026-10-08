// node web-shot.mjs <name> [route] [--actions file.json] [--width W --height H] [--light]
// route defaults to the #general channel; "{server}", "{general}" etc. are filled from state.json.
import { execSync } from "node:child_process";
import { createRequire } from "node:module";
import { readFileSync, mkdirSync } from "node:fs";
// Playwright from wherever it's installed: here, or globally (npm i -g playwright; npx playwright install chromium).
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
const opt = (k, d) => { const i = args.indexOf(k); return i >= 0 ? args[i + 1] : d; };
const name = args[0] ?? "shot";
let route = args[1] && !args[1].startsWith("--") ? args[1] : "/{instance}/{server}/{general}";
const fill = (s) => s.replace(/\{(\w+)\}/g, (_, k) => k === "instance" ? encodeURIComponent(st.host) : k === "server" ? st.server : (st.channels[k] ?? st.ids[k] ?? k));
route = fill(route);
const width = +opt("--width", 1280), height = +opt("--height", 800);
const actions = opt("--actions") ? JSON.parse(readFileSync(opt("--actions"), "utf8")) : [];
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 1, colorScheme: args.includes("--light") ? "light" : "dark" });
const as = process.env.AS ?? "alice";
const saved = [{ url: st.url, active: st.ids[as], accounts: [{ userId: st.ids[as], token: st.tokens[as], username: as, displayName: as, avatarUrl: "" }] }];
await ctx.addInitScript((v) => { if (!localStorage.getItem("fuwa:accounts:v2")) localStorage.setItem("fuwa:accounts:v2", v); }, JSON.stringify(saved));
const page = await ctx.newPage();
await page.goto(st.url + route);
await page.waitForLoadState("networkidle").catch(() => {});
await page.waitForTimeout(2500);
for (const a of actions) {
  if (a.click) await page.click(fill(a.click));
  if (a.text) await page.getByText(fill(a.text), { exact: a.exact ?? false }).first().click();
  if (a.rclick) await page.click(fill(a.rclick), { button: "right" });
  if (a.hover) await page.hover(fill(a.hover));
  if (a.mouse) await page.mouse.move(a.mouse[0], a.mouse[1]);
  if (a.wheel) { await page.mouse.move(a.wheel[0], a.wheel[1]); await page.mouse.wheel(0, a.wheel[2]); }
  if (a.at) await page.mouse.click(a.at[0], a.at[1]);
  if (a.press) await page.keyboard.press(a.press);
  if (a.type) await page.keyboard.type(a.type);
  await page.waitForTimeout(a.wait ?? 700);
}
mkdirSync(`${vis}/out`, { recursive: true });
await page.screenshot({ path: `${vis}/out/${name}-web.png` });
await browser.close();
console.log(`${vis}/out/${name}-web.png`);
