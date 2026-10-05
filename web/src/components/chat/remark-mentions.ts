/**
 * Finding mentions in message text: `@username`, `@everyone`, `@here`, roles
 * written `<@&role id>` and server emoji (`<:name:id>`). `Mention` in
 * mentions.tsx draws what this finds.
 */

const ROLE = String.raw`<@&([0-9A-Za-z]{26})>`;
const EVERYONE = String.raw`(?<![\w@<])@(everyone|here)\b`;
const USER = String.raw`(?<![\w@<.])@([a-z0-9][a-z0-9_.]{0,30}[a-z0-9_]|[a-z0-9])(?![\w])`;
const EMOJI = String.raw`<(a?):([A-Za-z0-9_]{2,32}):([0-9A-Za-z]{10,32})>`;
const PATTERN = new RegExp(`${ROLE}|${EVERYONE}|${USER}|${EMOJI}`, "gi");

type MdNode = { type: string; value?: string; children?: MdNode[]; data?: Record<string, unknown> };

const mention = (kind: string, target: string, text: string): MdNode => ({
  type: "mention",
  data: { hName: "fuwa-mention", hProperties: { dataKind: kind, dataTarget: target } },
  children: [{ type: "text", value: text }],
});

function split(value: string): MdNode[] | null {
  const out: MdNode[] = [];
  let last = 0;
  for (const m of value.matchAll(PATTERN)) {
    const at = m.index ?? 0;
    if (at > last) out.push({ type: "text", value: value.slice(last, at) });
    if (m[6]) out.push(mention("emoji", m[6].toUpperCase(), `:${m[5]}:`));
    else if (m[1]) out.push(mention("role", m[1].toUpperCase(), m[0]));
    else if (m[2]) out.push(mention("everyone", m[2].toLowerCase(), m[0]));
    else out.push(mention("user", m[3]!.toLowerCase(), m[0]));
    last = at + m[0].length;
  }
  if (!out.length) return null;
  if (last < value.length) out.push({ type: "text", value: value.slice(last) });
  return out;
}

/** Leaves code and links alone. */
const SKIP = new Set(["code", "inlineCode", "link", "linkReference", "html"]);

function walk(node: MdNode) {
  if (!node.children) return;
  const next: MdNode[] = [];
  for (const child of node.children) {
    if (child.type === "text" && child.value) {
      const parts = split(child.value);
      if (parts) {
        next.push(...parts);
        continue;
      }
    } else if (!SKIP.has(child.type)) walk(child);
    next.push(child);
  }
  node.children = next;
}

/** The remark plugin that turns mentions into `fuwa-mention` elements. */
export function remarkMentions() {
  return (tree: MdNode) => walk(tree);
}
