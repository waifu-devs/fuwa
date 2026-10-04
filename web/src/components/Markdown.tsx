import { memo, type ComponentProps, type ReactElement } from "react";
import renderMarkdown, { type Components, type Options } from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import { Timestamp } from "@/components/Timestamp";
import { remarkTimestamps } from "@/lib/timestamps";
import { cn } from "@/lib/utils";

/*
 * The one Markdown renderer for anything members write: bios, statuses, post
 * bodies, comments, theme descriptions. It's GitHub-flavored (tables,
 * strikethrough, task lists, bare URLs become links).
 *
 * Safety comes from react-markdown's defaults: raw HTML in the text is never
 * rendered and unsafe URLs (javascript:, data:, ...) are dropped. On top of
 * that, images show as links, so nobody can put tracking pixels or huge
 * pictures on someone else's page.
 *
 * Timestamps (`<t:SECONDS:STYLE>`, as in Discord) read in each reader's own
 * time zone, in block and inline Markdown alike.
 *
 * Styles live in app.css (.markdown and .markdown-inline) and only use theme
 * tokens, so Markdown looks right in every theme.
 */

type AnchorProps = ComponentProps<"a"> & { node?: unknown };

/** Where a link really goes, read as the browser would: "//host" and "/path" included. */
function resolve(href: string): URL | null {
  try {
    const url = new URL(href, location.href);
    return ["http:", "https:", "mailto:"].includes(url.protocol) ? url : null;
  } catch {
    return null;
  }
}

/**
 * Links leaving the site open in a new tab and don't pass on ranking or the
 * referrer. A link whose URL was dropped as unsafe is just its text.
 */
function Anchor({ node: _node, href, ...props }: AnchorProps) {
  const url = href ? resolve(href) : null;
  if (!url) return <span>{props.children}</span>;
  const external = url.origin !== location.origin;
  return <a href={url.href} {...(external ? { rel: "nofollow ugc noopener noreferrer", target: "_blank" } : {})} {...props} />;
}

const blockComponents: Components = {
  a: Anchor,
  img: ({ src, alt }) => <Anchor href={typeof src === "string" ? src : undefined}>{alt || "image"}</Anchor>,
  // Headings in member text shouldn't outrank the page's own.
  h1: ({ node: _node, ...props }) => <h3 {...props} />,
  h2: ({ node: _node, ...props }) => <h3 {...props} />,
  h3: ({ node: _node, ...props }) => <h4 {...props} />,
  h4: ({ node: _node, ...props }) => <h4 {...props} />,
  h5: ({ node: _node, ...props }) => <h4 {...props} />,
  h6: ({ node: _node, ...props }) => <h4 {...props} />,
  "fuwa-time": Timestamp,
} as Components;

/** Extra syntax for one kind of text, like mentions in chat: more remark plugins and the elements they make. */
export type MarkdownExtension = { remarkPlugins: NonNullable<Options["remarkPlugins"]>; components: Record<string, unknown> };

/*
 * Parsing is the expensive part (a chat channel parses every message it
 * shows), and the same text always makes the same elements, so they're kept:
 * a channel opened again, or a message list re-rendering for a new arrival,
 * reuses them. react-markdown's `Markdown` has no hooks, so it can be called
 * as a plain function; what it returns are ordinary React elements, and the
 * components inside them (links, mentions) still render live.
 */
const CACHE_SIZE = 2000;
const caches = new Map<string, Map<string, ReactElement>>();

function cached(kind: string, text: string, make: () => ReactElement): ReactElement {
  let cache = caches.get(kind);
  if (!cache) caches.set(kind, (cache = new Map()));
  const hit = cache.get(text);
  if (hit) {
    // Most recently used goes last, so the oldest is first to go.
    cache.delete(text);
    cache.set(text, hit);
    return hit;
  }
  const made = make();
  cache.set(text, made);
  if (cache.size > CACHE_SIZE) cache.delete(cache.keys().next().value!);
  return made;
}

const BLOCK_PLUGINS: NonNullable<Options["remarkPlugins"]> = [remarkGfm, remarkBreaks, remarkTimestamps];
const extensionIds = new WeakMap<MarkdownExtension, string>();
let extensions = 0;
const extensionOptions = (extension: MarkdownExtension): Options => ({
  remarkPlugins: [...BLOCK_PLUGINS, ...extension.remarkPlugins],
  components: { ...blockComponents, ...extension.components } as Components,
});
const extensionKey = (extension: MarkdownExtension) => {
  let id = extensionIds.get(extension);
  if (!id) extensionIds.set(extension, (id = `block:${++extensions}`));
  return id;
};

/** Block Markdown: paragraphs, lists, quotes, code blocks. A single newline is a line break. */
export const Markdown = memo(function Markdown({
  children,
  className,
  extension,
}: {
  children: string;
  className?: string;
  extension?: MarkdownExtension;
}) {
  const content = extension
    ? cached(extensionKey(extension), children, () => renderMarkdown({ ...extensionOptions(extension), children }))
    : cached("block", children, () => renderMarkdown({ remarkPlugins: BLOCK_PLUGINS, components: blockComponents, children }));
  return <div className={cn("markdown", className)}>{content}</div>;
});

const INLINE = ["em", "strong", "del", "code", "a", "fuwa-time"];
const INLINE_NO_LINKS = INLINE.filter((tag) => tag !== "a");
const INLINE_PLUGINS: NonNullable<Options["remarkPlugins"]> = [remarkGfm, remarkTimestamps];
const INLINE_COMPONENTS = { a: Anchor, "fuwa-time": Timestamp } as Components;

/**
 * Markdown for one-line text: bold, italics, strikethrough, code and links.
 * Anything block-level is flattened to its text. Pass `links={false}` inside
 * something that is already a link.
 */
export const InlineMarkdown = memo(function InlineMarkdown({
  children,
  className,
  links = true,
}: {
  children: string;
  className?: string;
  links?: boolean;
}) {
  const content = cached(links ? "inline" : "inline-no-links", children, () =>
    renderMarkdown({
      remarkPlugins: INLINE_PLUGINS,
      allowedElements: links ? INLINE : INLINE_NO_LINKS,
      unwrapDisallowed: true,
      components: INLINE_COMPONENTS,
      children,
    }),
  );
  return <span className={cn("markdown-inline", className)}>{content}</span>;
});
