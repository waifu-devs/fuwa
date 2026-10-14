/*
 * Math in Markdown: `$inline$`, `$$display$$` and ```math / ```latex code
 * blocks, drawn by KaTeX (components/Math.tsx). The desktop's `core/math.rs`
 * is this file in Rust, rule for rule, so a message reads the same in both.
 *
 * Which dollars are math is decided on the source, before it's parsed, so
 * prices stay prices: a single `$` opens only when a non-space follows it and
 * closes only after a non-space and before anything but a digit, on the same
 * line ("$5 and $10" is text, "$x^2$" is math). `$$` pairs may run over
 * lines but not over a blank one. Code, fenced or inline, and `\$` are left
 * alone.
 *
 * Each formula found becomes an inline code span whose text starts with a
 * private-use mark, so every later step (spoilers, mentions, search hits)
 * treats it as code and leaves its insides alone, and `remarkMath` turns the
 * marked spans into math nodes. A `$$` pair alone in its paragraph is a
 * display formula, centered on its own line; one amid text is drawn inline in
 * display style. Unpaired `$$` are escaped, so a parser never reads them as a
 * block that runs to the end of the message.
 */

/** Starts an inline formula's code span. */
export const INLINE_MARK = "";
/** Starts a `$$` formula's code span. */
export const DISPLAY_MARK = "";

/** The marks above, and search's match marks (lib/search-query.ts), which never belong in TeX. */
const MARKS = /[]/g;
const FENCE = /^(?:[ \t]*>)*[ \t]*(`{3,}|~{3,})/;
const PUNCT = /[!-/:-@[-`{-~]/;
const SPACE = /\s/;
const DIGIT = /[0-9]/;

/** The source with every formula made a marked code span (see above). */
export function markMath(source: string): string {
  if (!source.includes("$")) return source;
  const out: string[] = [];
  let text: string[] = [];
  let fence: string | null = null;
  const flush = () => {
    if (text.length) out.push(scan(text.join("\n")));
    text = [];
  };
  for (const line of source.split("\n")) {
    const opens = FENCE.exec(line)?.[1];
    if (fence) {
      out.push(line);
      if (opens && opens[0] === fence[0] && opens.length >= fence.length) fence = null;
    } else if (opens) {
      flush();
      fence = opens;
      out.push(line);
    } else {
      text.push(line);
    }
  }
  flush();
  return out.join("\n");
}

/** How many `ch` in a row start at `at`. */
function runAt(s: string, at: number, ch: string): number {
  let n = 0;
  while (s[at + n] === ch) n++;
  return n;
}

/** Where the next run of exactly `n` `ch` starts after `from`, or -1; `$` stops at the line's end (`line`) or a blank line. */
function closing(s: string, from: number, ch: string, n: number, line: boolean): number {
  let i = from;
  while (i < s.length) {
    const c = s[i]!;
    if (c === "\n") {
      if (line && ch === "$") return -1;
      const end = s.indexOf("\n", i + 1);
      if (!s.slice(i + 1, end < 0 ? s.length : end).replace(/[ \t>]/g, "")) return -1;
    } else if (ch === "$" && c === "\\" && s[i + 1] === "$") {
      // `\$` is a dollar sign in TeX, not the end.
      i += 2;
      continue;
    } else if (c === ch) {
      const run = runAt(s, i, ch);
      if (run === n) return i;
      i += run;
      continue;
    }
    i++;
  }
  return -1;
}

/** How many `>` quote the line `at` is on. */
function quoteDepth(s: string, at: number): number {
  const start = s.lastIndexOf("\n", at - 1) + 1;
  return (/^[ \t>]*/.exec(s.slice(start, at))?.[0].match(/>/g) ?? []).length;
}

/** A formula's TeX on one line: quote marks of its lines and `%` comments taken out, spaces run together. */
function texOf(content: string, depth: number): string {
  return content
    .replace(MARKS, "")
    .split("\n")
    .map((line, n) => {
      if (n > 0 && depth > 0) line = line.replace(new RegExp(`^(?:[ \\t]*>){0,${depth}}`), "");
      const comment = /(?<!\\)%/.exec(line);
      return comment ? line.slice(0, comment.index) : line;
    })
    .join(" ")
    .replace(/\s+/g, " ")
    .trim();
}

/** A formula as a marked code span, its fence longer than any backticks inside. */
function span(tex: string, display: boolean): string {
  const longest = Math.max(0, ...(tex.match(/`+/g) ?? []).map((r) => r.length));
  const fence = "`".repeat(longest + 1);
  return `${fence}${display ? DISPLAY_MARK : INLINE_MARK}${tex}${tex.endsWith("`") ? " " : ""}${fence}`;
}

/** One stretch of text between code blocks, its formulas marked. */
function scan(s: string): string {
  let out = "";
  let i = 0;
  while (i < s.length) {
    const c = s[i]!;
    if (c === "\\" && PUNCT.test(s[i + 1] ?? "")) {
      out += s.slice(i, i + 2);
      i += 2;
    } else if (c === "`") {
      const n = runAt(s, i, "`");
      const end = closing(s, i + n, "`", n, false);
      const to = end < 0 ? i + n : end + n;
      out += s.slice(i, to);
      i = to;
    } else if (c === "$") {
      const n = runAt(s, i, "$");
      const end = n <= 2 ? closing(s, i + n, "$", n, n === 1) : -1;
      const content = end < 0 ? "" : s.slice(i + n, end);
      const fits =
        end >= 0 &&
        (n === 2 ||
          (!SPACE.test(content[0] ?? " ") && !SPACE.test(content.at(-1) ?? " ") && !DIGIT.test(s[end + 1] ?? "")));
      const tex = fits ? texOf(content, quoteDepth(s, i)) : "";
      if (tex) {
        out += span(tex, n === 2);
        i = end + n;
      } else {
        out += n === 1 ? "$" : "\\$".repeat(n);
        i += n;
      }
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

/*
 * Formulas that define commands (`\def`, `\newcommand` and the like) aren't
 * drawn, so nobody's message runs macros of its own; nor is anything very
 * long. Both apps refuse the same ones.
 */
const DEFINES =
  /\\(?:[egx]?def|let|futurelet|global|(?:re)?newcommand|providecommand|(?:re)?newenvironment|DeclareMathOperator|csname|expandafter)(?![a-zA-Z])/;
export const MAX_TEX = 2000;

/** Whether a formula may be drawn. */
export function mathAllowed(tex: string): boolean {
  return tex.length <= MAX_TEX && !DEFINES.test(tex);
}

type MdNode = { type: string; value?: string; lang?: string | null; children?: MdNode[]; data?: Record<string, unknown> };

const LANGS = new Set(["math", "latex"]);
const node = (hName: string, value: string): MdNode => ({ type: "fuwaMath", value, data: { hName } });

/** The formula an inline code span holds, if `markMath` made it. */
function formula(child: MdNode): { tex: string; display: boolean } | null {
  if (child.type !== "inlineCode" || !child.value) return null;
  const mark = child.value[0];
  if (mark !== INLINE_MARK && mark !== DISPLAY_MARK) return null;
  const tex = child.value.slice(1).trim();
  return tex ? { tex, display: mark === DISPLAY_MARK } : null;
}

/** A paragraph that's nothing but one `$$` formula. */
function alone(paragraph: MdNode): string | null {
  const parts = (paragraph.children ?? []).filter((c) => !(c.type === "text" && !c.value?.trim()) && c.type !== "break");
  const only = parts.length === 1 ? formula(parts[0]!) : null;
  return only?.display ? only.tex : null;
}

function walk(parent: MdNode) {
  if (!parent.children) return;
  parent.children = parent.children.map((child) => {
    if (child.type === "code" && LANGS.has((child.lang ?? "").toLowerCase()) && child.value?.trim()) {
      return node("fuwa-math-block", child.value.trim());
    }
    if (child.type === "paragraph") {
      const tex = alone(child);
      if (tex) return node("fuwa-math-block", tex);
    }
    const found = formula(child);
    if (found) return node(found.display ? "fuwa-math-wide" : "fuwa-math", found.tex);
    walk(child);
    return child;
  });
}

/**
 * The remark plugin that turns marked code spans into `fuwa-math` (inline),
 * `fuwa-math-wide` (inline, display style) and `fuwa-math-block` elements,
 * and ```math / ```latex blocks into `fuwa-math-block`.
 */
export function remarkMath() {
  return (tree: MdNode) => walk(tree);
}
