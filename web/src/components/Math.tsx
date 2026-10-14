import { useEffect, useState, type ReactNode } from "react";
import type { KatexOptions } from "katex";
import { mathAllowed } from "@/lib/math";

/*
 * A formula in Markdown (`fuwa-math`, `fuwa-math-wide`, `fuwa-math-block`,
 * made by `remarkMath` in lib/math.ts), drawn by KaTeX. KaTeX and its fonts
 * are part of the app, in their own files that load the first time a formula
 * is on screen; until then, and for formulas that aren't drawn (one that
 * defines commands, or is too long), the TeX shows as it was written.
 *
 * KaTeX runs with `trust` off, so nothing a formula says can make a link,
 * load a picture or set a class or style; sizes and expansions are capped so
 * no formula can cover the page or take forever.
 */

type Katex = typeof import("katex").default;

let katex: Katex | null = null;
let loading: Promise<Katex> | null = null;

function load(): Promise<Katex> {
  loading ??= Promise.all([import("katex"), import("katex/dist/katex.min.css")]).then(([module]) => (katex = module.default));
  return loading;
}

const OPTIONS: KatexOptions = {
  throwOnError: false,
  trust: false,
  strict: "ignore",
  maxSize: 20,
  maxExpand: 1000,
  errorColor: "var(--destructive)",
};

type Mode = "inline" | "wide" | "block";

/** What KaTeX made of each formula, most recently used last. */
const made = new Map<string, string>();

function draw(k: Katex, tex: string, mode: Mode): string {
  const key = `${mode}:${tex}`;
  let html = made.get(key);
  if (html === undefined) {
    // `macros` is new each time: a formula's own definitions never outlive it.
    html = k.renderToString(mode === "wide" ? `\\displaystyle ${tex}` : tex, { ...OPTIONS, displayMode: mode === "block", macros: {} });
    if (made.size >= 500) made.delete(made.keys().next().value!);
  } else {
    made.delete(key);
  }
  made.set(key, html);
  return html;
}

function texOf(children: ReactNode): string {
  return (Array.isArray(children) ? children : [children]).filter((c) => typeof c === "string").join("");
}

function Formula({ children, mode }: { children?: ReactNode; mode: Mode }) {
  const tex = texOf(children);
  const allowed = mathAllowed(tex);
  const [k, setK] = useState(katex);
  useEffect(() => {
    if (allowed && !k) void load().then(setK);
  }, [allowed, k]);
  if (!allowed || !k) {
    const source = <code className="math-source">{mode === "inline" ? `$${tex}$` : `$$${tex}$$`}</code>;
    return mode === "block" ? <div className="math-block">{source}</div> : source;
  }
  const html = { __html: draw(k, tex, mode) };
  return mode === "block" ? <div className="math-block" dangerouslySetInnerHTML={html} /> : <span className="math" dangerouslySetInnerHTML={html} />;
}

type Props = { children?: ReactNode; node?: unknown };

export const InlineMath = ({ children }: Props) => <Formula mode="inline">{children}</Formula>;
export const WideMath = ({ children }: Props) => <Formula mode="wide">{children}</Formula>;
export const BlockMath = ({ children }: Props) => <Formula mode="block">{children}</Formula>;
