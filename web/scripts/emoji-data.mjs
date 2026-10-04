// Writes src/lib/emoji-data.json, the standard emoji the pickers offer, from
// emojibase-data (a dev dependency): `node scripts/emoji-data.mjs`. It's
// bundled with the app, so no emoji set is ever fetched from anywhere.
//
// Only emoji up to MAX_VERSION, which the systems people use draw by now.
// Each group is [id, name, entries]; each entry is
// [emoji, names (space-separated, the first is how it's written), words people
// search for, and for ones with skin tones the five toned ones].

import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";

const MAX_VERSION = 15.1;
const require = createRequire(import.meta.url);
const data = require("emojibase-data/en/data.json");
const github = require("emojibase-data/en/shortcodes/github.json");
const emojibase = require("emojibase-data/en/shortcodes/emojibase.json");

const GROUPS = [
  ["people", "Smileys & people", [0, 1]],
  ["nature", "Animals & nature", [3]],
  ["food", "Food & drink", [4]],
  ["activities", "Activities", [6]],
  ["travel", "Travel & places", [5]],
  ["objects", "Objects", [7]],
  ["symbols", "Symbols", [8]],
  ["flags", "Flags", [9]],
];

const list = (v) => (v === undefined ? [] : Array.isArray(v) ? v : [v]);
const word = (s) => s.toLowerCase().replace(/[^a-z0-9_+-]+/g, "_").replace(/^_+|_+$/g, "");
const taken = new Set();

const out = GROUPS.map(([id, name, groups]) => {
  const entries = data
    .filter((e) => groups.includes(e.group) && e.version <= MAX_VERSION)
    .sort((a, b) => a.order - b.order)
    .map((e) => {
      const names = [...new Set([...list(github[e.hexcode]), ...list(emojibase[e.hexcode])].map(word).filter(Boolean))];
      if (!names.length) names.push(word(e.label));
      // Each name finds one emoji.
      const own = names.filter((n) => !taken.has(n));
      own.forEach((n) => taken.add(n));
      if (!own.length) own.push(`${names[0]}_${e.hexcode.toLowerCase()}`);
      const words = [...new Set([...(e.tags ?? []), ...word(e.label).split("_")])].filter((w) => !own.includes(w)).join(" ");
      const skins = (e.skins ?? []).filter((s) => typeof s.tone === "number" && s.version <= MAX_VERSION).sort((a, b) => a.tone - b.tone);
      const entry = [e.emoji, own.join(" "), words];
      if (skins.length === 5) entry.push(skins.map((s) => s.emoji));
      return entry;
    });
  return [id, name, entries];
});

const path = new URL("../src/lib/emoji-data.json", import.meta.url);
const json = `${JSON.stringify(out)}\n`;
const before = (() => {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return "";
  }
})();
writeFileSync(path, json);
const count = out.reduce((n, [, , e]) => n + e.length, 0);
console.log(`${count} emoji, ${(json.length / 1024).toFixed(0)} KB${before === json ? " (unchanged)" : ""}`);
