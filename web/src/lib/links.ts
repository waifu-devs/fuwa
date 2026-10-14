/*
 * Masked links: Markdown links whose words aren't their address
 * (`[free nitro](https://phish.example)`). Opening one asks first, showing
 * where it really goes (components/LeavingDialog.tsx), unless its site is one
 * the person trusts (`Prefs.trustedDomains`, this device's only). Bare links
 * and links whose words are their own address open at once.
 */

type MdNode = { type: string; value?: string; url?: string; children?: MdNode[]; data?: { hProperties?: Record<string, unknown> } & Record<string, unknown> };

/** The words of a node, as read. */
function wordsOf(node: MdNode): string {
  if (node.type === "text" || node.type === "inlineCode") return node.value ?? "";
  return (node.children ?? []).map(wordsOf).join("");
}

function parse(text: string): URL | null {
  try {
    return new URL(/^[a-z][a-z0-9+.-]*:/i.test(text) ? text : `https://${text}`);
  } catch {
    return null;
  }
}

/** The host a link goes to, lowercased, without "www.". */
export function hostOf(href: string): string {
  const url = parse(href);
  return url ? url.hostname.toLowerCase().replace(/^www\./, "") : "";
}

/**
 * Whether a link's words hide where it goes: they aren't its address, nor
 * its address without the scheme or a trailing slash. Words that are another
 * address entirely (`[https://bank.example](https://phish.example)`) are the
 * worst kind, and masked too.
 */
export function isMasked(words: string, href: string): boolean {
  const said = words.trim();
  if (!said || said === href) return false;
  const a = parse(said);
  const b = parse(href);
  if (!a || !b || !/[./]/.test(said)) return true;
  // Words without a scheme name the address without one.
  if (/^[a-z][a-z0-9+.-]*:/i.test(said) && a.protocol !== b.protocol) return true;
  const plain = (u: URL) => `${u.host}${u.pathname.replace(/\/$/, "")}${u.search}${u.hash}`;
  return plain(a) !== plain(b);
}

/** Whether `host` is one of the trusted sites, or under one. */
export function trusted(domains: readonly string[], host: string): boolean {
  return !!host && domains.some((d) => host === d || host.endsWith(`.${d}`));
}

/** A site as kept in the trusted list: its host, lowercased, without "www."; "" if it isn't one. */
export function cleanDomain(input: string): string {
  return hostOf(input.trim());
}

/** The remark plugin that marks masked links (`data-masked` on the `a`). */
export function remarkMaskedLinks() {
  const walk = (node: MdNode) => {
    for (const child of node.children ?? []) {
      if (child.type === "link" && child.url && isMasked(wordsOf(child), child.url)) {
        child.data = { ...child.data, hProperties: { ...child.data?.hProperties, dataMasked: "true" } };
      }
      walk(child);
    }
  };
  return (tree: MdNode) => walk(tree);
}
