import { XIcon } from "lucide-react";
import { m as motion } from "motion/react";
import type { Embed } from "@/gen/fuwa/v1/types_pb";
import { guardMasked } from "@/components/LeavingDialog";
import { InlineMarkdown, Markdown } from "@/components/Markdown";
import { useI18n } from "@/i18n/react";
import { colorCss } from "@/lib/format";
import { shownPicture } from "@/lib/shown";
import { cn } from "@/lib/utils";

/** Only links a browser should open: http(s). */
const safe = (url: string) => (/^https?:\/\//i.test(url) ? url : "");

/**
 * Rich cards under a message, as apps post them through webhooks: a colored
 * edge, a title that can link somewhere, Markdown, fields side by side, a
 * thumbnail and a big picture. Link previews (`embed.preview`, made by the
 * instance from the page a link points to) are drawn plainer, their words
 * as plain text; `onHide`, on your own messages, takes them off.
 */
export function Embeds({ embeds, animate, onHide }: { embeds: Embed[]; animate: boolean; onHide?: () => void }) {
  if (!embeds.length) return null;
  return (
    <div className="mt-1 flex flex-col gap-1.5">
      {embeds.map((embed, n) =>
        embed.preview ? (
          <LinkPreview key={embed.url} embed={embed} delay={animate ? 0.08 + n * 0.06 : 0} animate={animate} onHide={onHide} />
        ) : (
          <EmbedCard key={n} embed={embed} delay={animate ? 0.08 + n * 0.06 : 0} animate={animate} />
        ),
      )}
    </div>
  );
}

/** A link preview: the site, the page's title as the link, its description and picture. */
function LinkPreview({ embed, delay, animate, onHide }: { embed: Embed; delay: number; animate: boolean; onHide?: () => void }) {
  const { t } = useI18n();
  const url = safe(embed.url);
  const thumbnail = shownPicture(safe(embed.thumbnailUrl));
  const image = shownPicture(safe(embed.imageUrl));
  const words = !!(embed.title || embed.description);
  if (!url || (!words && !image)) return null;
  const edge = embed.color ? colorCss(embed.color) : "var(--border)";
  const hide = onHide && (
    <motion.button
      type="button"
      aria-label={t("chat.messages.hidePreviews")}
      title={t("chat.messages.hidePreviews")}
      onClick={onHide}
      whileHover={{ scale: 1.1 }}
      whileTap={{ scale: 0.9 }}
      className="absolute top-1.5 right-1.5 z-10 grid size-6 place-items-center rounded-full bg-background/80 text-muted-foreground opacity-0 shadow-sm transition-opacity group-hover/preview:opacity-100 hover:text-foreground focus-visible:opacity-100"
    >
      <XIcon className="size-3.5" />
    </motion.button>
  );
  return (
    <motion.div
      initial={animate ? { opacity: 0, y: 6, scale: 0.98 } : false}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.98 }}
      transition={{ type: "spring", stiffness: 460, damping: 30, delay }}
      style={words ? { borderLeftColor: edge } : undefined}
      className={cn("group/preview relative max-w-md overflow-hidden rounded-xl text-sm", words && "border border-l-4 bg-card/70 p-3 shadow-sm")}
    >
      {hide}
      {words ? (
        <>
          <div className="flex gap-3">
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              {embed.siteName && <p className="truncate text-xs text-muted-foreground">{embed.siteName}</p>}
              {embed.title && (
                <a href={url} target="_blank" rel="noreferrer noopener" className="line-clamp-2 font-extrabold break-words text-primary hover:underline">
                  {embed.title}
                </a>
              )}
              {embed.description && <p className="line-clamp-3 text-[0.9em] break-words whitespace-pre-line text-muted-foreground">{embed.description}</p>}
            </div>
            {thumbnail && <img src={thumbnail} alt="" loading="lazy" className="size-16 shrink-0 rounded-lg object-cover" />}
          </div>
          {image && (
            <a href={url} target="_blank" rel="noreferrer noopener" className="mt-2 block">
              <img src={image} alt="" loading="lazy" className="max-h-72 w-full rounded-lg object-cover" />
            </a>
          )}
        </>
      ) : (
        <a href={url} target="_blank" rel="noreferrer noopener" className="block">
          <img src={image} alt="" loading="lazy" className="max-h-80 max-w-full rounded-xl object-contain" />
        </a>
      )}
    </motion.div>
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
              <a
                href={url}
                target="_blank"
                rel="noreferrer noopener"
                // An app's title is words over an address: ask before going.
                onClick={(event) => guardMasked(event, url)}
                className="font-extrabold text-primary hover:underline"
              >
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
