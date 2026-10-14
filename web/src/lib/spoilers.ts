/*
 * Spoilers: text between `||` pairs, as in Discord, hidden until the reader
 * reveals it. The remark plugin turns each pair, within one paragraph, into a
 * `fuwa-spoiler` element around whatever is between them (bold, links,
 * mentions, emoji and line breaks included); code is left alone. A `||`
 * without a partner, or `||||`, stays as it is.
 */

type MdNode = { type: string; value?: string; url?: string; children?: MdNode[]; data?: Record<string, unknown> };

const MARK = "||";
const marker = (): MdNode => ({ type: "spoilerMark" });
const isMark = (node: MdNode) => node.type === "spoilerMark";

/** Leaves code alone, and spoilers already made. */
const SKIP = new Set(["code", "inlineCode", "html", "spoiler"]);

/** Splits a text node at its markers. */
function splitText(value: string): MdNode[] {
  const out: MdNode[] = [];
  value.split(MARK).forEach((part, n) => {
    if (n > 0) out.push(marker());
    if (part) out.push({ type: "text", value: part });
  });
  return out;
}

/**
 * A bare link written right before a closing marker (`||https://x.example||`)
 * takes the marker into its address; it's given back.
 */
function splitLink(link: MdNode): MdNode[] | null {
  const text = link.children?.length === 1 ? link.children[0]! : null;
  if (!link.url?.endsWith(MARK) || text?.type !== "text" || !text.value?.endsWith(MARK)) return null;
  const url = link.url.slice(0, -MARK.length);
  const value = text.value.slice(0, -MARK.length);
  if (!value) return [marker()];
  return [{ ...link, url, children: [{ ...text, value }] }, marker()];
}

/** One parent's children with each pair of markers made a spoiler around what's between. */
function pair(children: MdNode[]): MdNode[] {
  const flat: MdNode[] = [];
  for (const child of children) {
    if (child.type === "text" && child.value?.includes(MARK)) flat.push(...splitText(child.value));
    else if (child.type === "link") flat.push(...(splitLink(child) ?? [child]));
    else flat.push(child);
  }
  if (!flat.some(isMark)) return children;
  const out: MdNode[] = [];
  let open = -1;
  for (const node of flat) {
    if (!isMark(node)) {
      out.push(node);
    } else if (open < 0) {
      open = out.length;
      out.push(node);
    } else {
      const inner = out.splice(open + 1);
      out.pop();
      out.push(inner.length ? { type: "spoiler", data: { hName: "fuwa-spoiler" }, children: inner } : { type: "text", value: MARK + MARK });
      open = -1;
    }
  }
  return merge(out.map((node) => (isMark(node) ? { type: "text", value: MARK } : node)));
}

/** Neighbouring text nodes as one, as the parser would have made them. */
function merge(nodes: MdNode[]): MdNode[] {
  const out: MdNode[] = [];
  for (const node of nodes) {
    const last = out.at(-1);
    if (node.type === "text" && last?.type === "text") out[out.length - 1] = { ...last, value: (last.value ?? "") + (node.value ?? "") };
    else out.push(node);
  }
  return out;
}

function walk(node: MdNode) {
  if (!node.children) return;
  for (const child of node.children) if (!SKIP.has(child.type)) walk(child);
  node.children = pair(node.children);
}

/** The remark plugin that turns `||...||` into `fuwa-spoiler` elements. */
export function remarkSpoilers() {
  return (tree: MdNode) => walk(tree);
}

/**
 * Text with its spoilers blanked out, for places that show a message as plain
 * text (a system notification): each becomes a block of shade, about as
 * long, so nothing hidden shows outside the app.
 */
export function hideSpoilers(text: string): string {
  return text.replace(/\|\|([\s\S]+?)\|\|/g, (_, hidden: string) => "▒".repeat(Math.min(Math.max(hidden.trim().length, 3), 12)));
}
