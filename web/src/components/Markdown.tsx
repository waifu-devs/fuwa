import type { ComponentProps } from "react";
import ReactMarkdown, { type Components, type Options } from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
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
};

/** Extra syntax for one kind of text, like mentions in chat: more remark plugins and the elements they make. */
export type MarkdownExtension = { remarkPlugins: NonNullable<Options["remarkPlugins"]>; components: Record<string, unknown> };

/** Block Markdown: paragraphs, lists, quotes, code blocks. A single newline is a line break. */
export function Markdown({ children, className, extension }: { children: string; className?: string; extension?: MarkdownExtension }) {
  return (
    <div className={cn("markdown", className)}>
      <ReactMarkdown
        remarkPlugins={extension ? [remarkGfm, remarkBreaks, ...extension.remarkPlugins] : [remarkGfm, remarkBreaks]}
        components={extension ? ({ ...blockComponents, ...extension.components } as Components) : blockComponents}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}

const INLINE = ["em", "strong", "del", "code", "a"];
const INLINE_NO_LINKS = INLINE.filter((tag) => tag !== "a");

/**
 * Markdown for one-line text: bold, italics, strikethrough, code and links.
 * Anything block-level is flattened to its text. Pass `links={false}` inside
 * something that is already a link.
 */
export function InlineMarkdown({ children, className, links = true }: { children: string; className?: string; links?: boolean }) {
  return (
    <span className={cn("markdown-inline", className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} allowedElements={links ? INLINE : INLINE_NO_LINKS} unwrapDisallowed components={{ a: Anchor }}>
        {children}
      </ReactMarkdown>
    </span>
  );
}
