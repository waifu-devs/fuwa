import { m as motion } from "motion/react";
import type { Embed } from "@/gen/fuwa/v1/types_pb";
import { InlineMarkdown, Markdown } from "@/components/Markdown";
import { colorCss } from "@/lib/format";
import { shownPicture } from "@/lib/shown";
import { cn } from "@/lib/utils";

/** Only links a browser should open: http(s). */
const safe = (url: string) => (/^https?:\/\//i.test(url) ? url : "");

/**
 * Rich cards under a message, as apps post them through webhooks: a colored
 * edge, a title that can link somewhere, Markdown, fields side by side, a
 * thumbnail and a big picture.
 */
export function Embeds({ embeds, animate }: { embeds: Embed[]; animate: boolean }) {
  if (!embeds.length) return null;
  return (
    <div className="mt-1 flex flex-col gap-1.5">
      {embeds.map((embed, n) => (
        <EmbedCard key={n} embed={embed} delay={animate ? 0.08 + n * 0.06 : 0} animate={animate} />
      ))}
    </div>
  );
}

function EmbedCard({ embed, delay, animate }: { embed: Embed; delay: number; animate: boolean }) {
  const url = safe(embed.url);
  // Pictures load only from fuwa instances, never straight from other sites.
  const thumbnail = shownPicture(safe(embed.thumbnailUrl));
  const image = shownPicture(safe(embed.imageUrl));
  const edge = embed.color ? colorCss(embed.color) : "var(--border)";
  return (
    <motion.div
      initial={animate ? { opacity: 0, x: -10, scale: 0.98 } : false}
      animate={{ opacity: 1, x: 0, scale: 1 }}
      whileHover={{ y: -1 }}
      transition={{ type: "spring", stiffness: 460, damping: 30, delay }}
      style={{ borderLeftColor: edge }}
      className="relative max-w-lg overflow-hidden rounded-xl border border-l-4 bg-card/70 p-3 text-sm shadow-sm"
    >
      <motion.span
        aria-hidden
        initial={animate ? { opacity: 0.35 } : { opacity: 0 }}
        animate={{ opacity: 0 }}
        transition={{ duration: 1.2, delay }}
        style={{ background: `linear-gradient(90deg, ${edge}, transparent 70%)` }}
        className="pointer-events-none absolute inset-0"
      />
      <div className="relative flex gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          {embed.title &&
            (url ? (
              <a href={url} target="_blank" rel="noreferrer noopener" className="font-extrabold text-primary hover:underline">
                <InlineMarkdown links={false}>{embed.title}</InlineMarkdown>
              </a>
            ) : (
              <span className="font-extrabold">
                <InlineMarkdown>{embed.title}</InlineMarkdown>
              </span>
            ))}
          {embed.description && <Markdown className="chat text-[0.95em]">{embed.description}</Markdown>}
          {embed.fields.length > 0 && (
            <div className="mt-1 grid grid-cols-1 gap-x-4 gap-y-2 sm:grid-cols-3">
              {embed.fields.map((field, n) => (
                <motion.div
                  key={n}
                  initial={animate ? { opacity: 0, y: 4 } : false}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ delay: delay + 0.1 + n * 0.03 }}
                  className={cn("min-w-0", !field.inline && "sm:col-span-3")}
                >
                  <p className="text-xs font-extrabold">
                    <InlineMarkdown>{field.name}</InlineMarkdown>
                  </p>
                  <Markdown className="chat text-[0.9em] text-muted-foreground">{field.value}</Markdown>
                </motion.div>
              ))}
            </div>
          )}
        </div>
        {thumbnail && <img src={thumbnail} alt="" loading="lazy" className="size-16 shrink-0 rounded-lg object-cover" />}
      </div>
      {image && <img src={image} alt="" loading="lazy" className="relative mt-2 max-h-72 w-full rounded-lg object-cover" />}
    </motion.div>
  );
}
