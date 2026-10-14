import assert from "node:assert/strict";
import { test } from "node:test";
import { hideSpoilers, remarkSpoilers } from "./spoilers.ts";

type Node = { type: string; value?: string; url?: string; children?: Node[]; data?: Record<string, unknown> };
const text = (value: string): Node => ({ type: "text", value });
const run = (children: Node[]) => {
  const tree: Node = { type: "root", children: [{ type: "paragraph", children }] };
  remarkSpoilers()(tree);
  return tree.children![0]!.children!;
};

test("text between pairs of || becomes a spoiler", () => {
  const parts = run([text("a ||secret|| b ||two||")]);
  assert.deepEqual(
    parts.map((n) => n.type),
    ["text", "spoiler", "text", "spoiler"],
  );
  assert.equal(parts[1]!.data!.hName, "fuwa-spoiler");
  assert.equal(parts[1]!.children![0]!.value, "secret");
  assert.equal(parts[2]!.value, " b ");
});

test("a spoiler wraps formatting, and leaves code alone", () => {
  const strong: Node = { type: "strong", children: [text("bold")] };
  const code: Node = { type: "inlineCode", value: "a || b" };
  const parts = run([text("||it's "), strong, text(" and "), code, text("||")]);
  assert.equal(parts.length, 1);
  assert.deepEqual(
    parts[0]!.children!.map((n) => n.type),
    ["text", "strong", "text", "inlineCode"],
  );
  assert.equal(code.value, "a || b");
});

test("a lone || or an empty pair stays as it was", () => {
  assert.deepEqual(run([text("a || b")]), [text("a || b")]);
  assert.deepEqual(run([text("x |||| y")]), [text("x |||| y")]);
  const parts = run([text("||one|| ||")]);
  assert.deepEqual(
    parts.map((n) => n.type),
    ["spoiler", "text"],
  );
  assert.equal(parts[1]!.value, " ||");
});

test("a bare link before the closing marker gives it back", () => {
  const link: Node = { type: "link", url: "https://x.example||", children: [text("https://x.example||")] };
  const parts = run([text("look ||"), link]);
  assert.deepEqual(
    parts.map((n) => n.type),
    ["text", "spoiler"],
  );
  const inner = parts[1]!.children![0]!;
  assert.equal(inner.url, "https://x.example");
  assert.equal(inner.children![0]!.value, "https://x.example");
});

test("notifications never show what a spoiler hides", () => {
  assert.equal(hideSpoilers("the ending: ||they win|| ok"), "the ending: ▒▒▒▒▒▒▒▒ ok");
  assert.equal(hideSpoilers("no spoilers || here"), "no spoilers || here");
});
