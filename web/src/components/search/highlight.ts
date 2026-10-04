import { MARK_CLOSE, MARK_OPEN } from "@/lib/search-query";

/**
 * The remark plugin that turns the marks `markRanges` put around search
 * matches into highlighted `<mark>`s. Marks inside code and link addresses
 * are taken out instead, so they never change what's shown or where a link
 * goes. A match split by formatting (half bold) stays highlighted across it.
 */

type MdNode = { type: string; value?: string; url?: string; children?: MdNode[]; data?: Record<string, unknown> };

const MARKS = new RegExp(`[${MARK_OPEN}${MARK_CLOSE}]`, "g");
const strip = (s: string) => s.replace(MARKS, "");

const hit = (value: string): MdNode => ({
  type: "searchHit",
  data: { hName: "mark", hProperties: { className: "search-hit" } },
  children: [{ type: "text", value }],
});

export function remarkSearchHits() {
  return (tree: MdNode) => {
    let open = false;
    const walk = (node: MdNode) => {
      if (node.url) node.url = strip(node.url);
      if (!node.children) return;
      const next: MdNode[] = [];
      for (const child of node.children) {
        if ((child.type === "code" || child.type === "inlineCode" || child.type === "html") && child.value) {
          child.value = strip(child.value);
        }
        if (child.type !== "text" || !child.value) {
          walk(child);
          next.push(child);
          continue;
        }
        let run = "";
        const flush = () => {
          if (run) next.push(open ? hit(run) : { type: "text", value: run });
          run = "";
        };
        for (const ch of child.value) {
          if (ch === MARK_OPEN || ch === MARK_CLOSE) {
            flush();
            open = ch === MARK_OPEN;
          } else run += ch;
        }
        flush();
      }
      node.children = next;
    };
    walk(tree);
  };
}
