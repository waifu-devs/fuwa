import assert from "node:assert/strict";
import { test } from "node:test";
import { DISPLAY_MARK as D, INLINE_MARK as I, markMath, mathAllowed, remarkMath } from "./math.ts";

// The same cases are in the desktop's core/math.rs: both apps must agree on what's math.

test("formulas become marked code spans", () => {
  assert.equal(markMath("so $x^2$ then"), `so \`${I}x^2\` then`);
  assert.equal(markMath("$$\\sum_i i$$"), `\`${D}\\sum_i i\``);
  assert.equal(markMath("a $x$ and $y$"), `a \`${I}x\` and \`${I}y\``);
});

test("prices stay text", () => {
  for (const text of ["costs $5 and $10", "$5-$10", "between $5 and $6.50", "US$5 vs US$6", "it's $ 5 or $ 6", "$5/$10 a month"]) {
    assert.equal(markMath(text), text, text);
  }
  assert.equal(markMath("I have $5"), "I have $5");
});

test("single dollars keep to one line, double ones to one paragraph", () => {
  assert.equal(markMath("$a\nb$"), "$a\nb$");
  assert.equal(markMath("$$\na + b\n$$"), `\`${D}a + b\``);
  assert.equal(markMath("$$a\n\nb$$"), "\\$\\$a\n\nb\\$\\$");
});

test("code and escapes are left alone", () => {
  assert.equal(markMath("`$x$` and \\$5 and \\$x$"), "`$x$` and \\$5 and \\$x$");
  assert.equal(markMath("```\n$x$\n```\n$y$"), `\`\`\`\n$x$\n\`\`\`\n\`${I}y\``);
  assert.equal(markMath("~~~tex\n$$a$$\n~~~"), "~~~tex\n$$a$$\n~~~");
});

test("a formula's lines join, without quote marks or comments", () => {
  assert.equal(markMath("> $$\n> a % note\n> + b\n> $$"), `> \`${D}a + b\``);
});

test("backticks inside a formula get a longer fence", () => {
  assert.equal(markMath("$a`b$"), `\`\`${I}a\`b\`\``);
});

test("search marks never reach the TeX", () => {
  assert.equal(markMath("$x$"), `\`${I}x\``);
});

test("unpaired double dollars are escaped, single ones stay", () => {
  assert.equal(markMath("$$ and $"), "\\$\\$ and $");
  assert.equal(markMath("$$$x$$$"), "\\$\\$\\$x\\$\\$\\$");
});

test("formulas that define commands aren't drawn", () => {
  assert.ok(mathAllowed("\\frac{a}{b}"));
  assert.ok(mathAllowed("\\deff"));
  assert.ok(!mathAllowed("\\def\\x{y}\\x"));
  assert.ok(!mathAllowed("\\newcommand{\\x}{y}"));
  assert.ok(!mathAllowed("\\gdef\\x{y}"));
  assert.ok(!mathAllowed("x".repeat(2001)));
});

type Node = { type: string; value?: string; lang?: string; children?: Node[]; data?: Record<string, unknown> };
const run = (children: Node[]) => {
  const tree: Node = { type: "root", children };
  remarkMath()(tree);
  return tree.children!;
};
const hName = (n: Node) => n.data?.hName;

test("marked spans become math, a lone $$ a block", () => {
  const [p] = run([
    {
      type: "paragraph",
      children: [
        { type: "inlineCode", value: `${I}x` },
        { type: "inlineCode", value: `${D}y` },
        { type: "inlineCode", value: "plain" },
      ],
    },
  ]);
  assert.deepEqual(p!.children!.map(hName), ["fuwa-math", "fuwa-math-wide", undefined]);
  assert.equal(p!.children![0]!.value, "x");
  const [block] = run([{ type: "paragraph", children: [{ type: "inlineCode", value: `${D}\\int f` }] }]);
  assert.equal(hName(block!), "fuwa-math-block");
  assert.equal(block!.value, "\\int f");
});

test("math and latex code blocks are drawn, others aren't", () => {
  const blocks = run([
    { type: "code", lang: "latex", value: "a^2\n" },
    { type: "code", lang: "math", value: "b" },
    { type: "code", lang: "tex", value: "c" },
  ]);
  assert.deepEqual(blocks.map(hName), ["fuwa-math-block", "fuwa-math-block", undefined]);
});
