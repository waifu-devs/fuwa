import assert from "node:assert/strict";
import { test } from "node:test";
import { formatFull, formatRelative, formatTimestamp, parseToken, refreshEvery, remarkTimestamps, toToken, TOKEN } from "./timestamps.ts";

// 2026-10-04 16:20:30 UTC, a Sunday.
const AT = Date.UTC(2026, 9, 4, 16, 20, 30);
const SECONDS = AT / 1000;
const fmt = (style: Parameters<typeof formatTimestamp>[1]) => formatTimestamp(AT, style, 0, "en-US", "UTC");

test("every style reads as Discord shows it", () => {
  assert.equal(fmt("t"), "4:20 PM");
  assert.equal(fmt("T"), "4:20:30 PM");
  assert.equal(fmt("d"), "10/04/2026");
  assert.equal(fmt("D"), "October 4, 2026");
  assert.match(fmt("f"), /^October 4, 2026,? (at )?4:20 PM$/);
  assert.match(fmt("F"), /^Sunday, October 4, 2026,? (at )?4:20 PM$/);
  assert.equal(formatFull(AT, "en-US", "UTC"), fmt("F"));
});

test("dates follow the reader's own language and time zone", () => {
  assert.equal(formatTimestamp(AT, "t", 0, "en-GB", "UTC"), "16:20");
  assert.equal(formatTimestamp(AT, "d", 0, "en-GB", "UTC"), "04/10/2026");
  assert.equal(formatTimestamp(AT, "t", 0, "en-US", "Asia/Tokyo"), "1:20 AM");
  assert.equal(formatTimestamp(AT, "D", 0, "en-US", "Asia/Tokyo"), "October 5, 2026");
});

test("relative times step through Discord's units", () => {
  const rel = (gap: number) => formatRelative(AT + gap, AT, "en-US");
  assert.equal(rel(0), "now");
  assert.equal(rel(5_000), "in 5 seconds");
  assert.equal(rel(-30_000), "30 seconds ago");
  assert.equal(rel(50_000), "in 1 minute");
  assert.equal(rel(3 * 3_600_000), "in 3 hours");
  assert.equal(rel(-2 * 86_400_000), "2 days ago");
  assert.equal(rel(23 * 3_600_000), "tomorrow");
  assert.equal(rel(60 * 86_400_000), "in 2 months");
  assert.equal(rel(-400 * 86_400_000), "last year");
  assert.equal(formatTimestamp(AT + 3 * 3_600_000, "R", AT, "en-US"), "in 3 hours");
});

test("relative times refresh every second, then every minute", () => {
  assert.equal(refreshEvery(AT + 10_000, AT), 1_000);
  assert.equal(refreshEvery(AT + 3_600_000, AT), 60_000);
});

test("tokens parse like Discord's, and nothing else does", () => {
  assert.deepEqual(parseToken(String(SECONDS)), { ms: AT, style: "f" });
  assert.deepEqual(parseToken(String(SECONDS), "R"), { ms: AT, style: "R" });
  assert.deepEqual(parseToken("-60", "t"), { ms: -60_000, style: "t" });
  assert.equal(parseToken(String(SECONDS), "x"), null);
  assert.equal(parseToken("99999999999999"), null);
  const found = [..."at <t:1759594830:R> or <t:1759594830> but not <t:12:x> or <t:abc>".matchAll(TOKEN)].map((m) => m[0]);
  assert.deepEqual(found, ["<t:1759594830:R>", "<t:1759594830>"]);
});

test("the picker writes tokens Discord reads", () => {
  assert.equal(toToken(new Date(AT), "R"), `<t:${SECONDS}:R>`);
  assert.equal(toToken(new Date(AT + 999), "t"), `<t:${SECONDS}:t>`);
  assert.equal(toToken(new Date(AT), "f"), `<t:${SECONDS}>`);
});

test("tokens in text become elements, but not inside code or links", () => {
  type Node = { type: string; value?: string; children?: Node[]; data?: { hProperties?: Record<string, string> } };
  const tree: Node = {
    type: "root",
    children: [
      {
        type: "paragraph",
        children: [
          { type: "text", value: "starts <t:1759594830:R>, ends <t:1759594830:x>" },
          { type: "inlineCode", value: "<t:1759594830>" },
          { type: "link", children: [{ type: "text", value: "<t:1759594830>" }] },
        ],
      },
    ],
  };
  remarkTimestamps()(tree);
  const parts = tree.children![0]!.children!;
  assert.deepEqual(
    parts.map((n) => n.type),
    ["text", "timestamp", "text", "inlineCode", "link"],
  );
  assert.deepEqual(parts[1]!.data!.hProperties, { dataTime: "1759594830", dataStyle: "R" });
  assert.equal(parts[2]!.value, ", ends <t:1759594830:x>");
  assert.equal(parts[4]!.children![0]!.type, "text");
});
