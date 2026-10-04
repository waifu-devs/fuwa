import { ArrowUpRightIcon, GiftIcon } from "lucide-react";
import { motion } from "motion/react";
import type { Node } from "@/gen/fuwa/v1/types_pb";
import { SPRING } from "@/components/motion";
import { formatStamp, toDate } from "@/lib/format";
import { reduceMotion } from "@/lib/prefs";

const GUIDE = "https://github.com/waifu-devs/fuwa/blob/master/docs/self-hosting.md#updating";

/**
 * "fuwa 0.4.2 is out": shown to the instance's admins when its daily check
 * (server/src/releases.rs) found a newer release. Nothing updates by itself;
 * the links say where to read about it and how. Both open github.com, and say so.
 */
export function NewerRelease({ node }: { node: Node | null | undefined }) {
  const release = node?.newerRelease;
  if (!release?.version) return null;
  const calm = reduceMotion();
  const stamp = release.publishedAt ? formatStamp(toDate(release.publishedAt)) : null;
  // Mid-sentence: "came out today at 1:00 AM".
  const when = stamp?.replace(/^(Today|Yesterday)\b/, (day) => day.toLowerCase()) ?? null;
  // Only GitHub's own release pages, whatever an instance says.
  const page = release.url.startsWith("https://github.com/waifu-devs/fuwa/releases/") ? release.url : null;
  return (
    <motion.div
      initial={calm ? { opacity: 0 } : { opacity: 0, y: 10, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={SPRING}
      className="mb-5 flex items-start gap-3 rounded-2xl border border-primary/30 bg-primary/8 p-4"
    >
      <motion.span
        className="grid size-10 shrink-0 place-items-center rounded-xl bg-primary/15 text-primary"
        initial={calm ? false : { rotate: -12, scale: 0.6 }}
        animate={{ rotate: 0, scale: 1 }}
        transition={{ type: "spring", stiffness: 380, damping: 14, delay: 0.08 }}
      >
        <GiftIcon className="size-5" />
      </motion.span>
      <div className="min-w-0 flex-1">
        <p className="font-bold">fuwa {release.version} is out</p>
        <p className="mt-0.5 text-sm text-muted-foreground">
          This instance runs {node?.version}
          {when ? `; ${release.version} came out ${when}` : ""}. Nothing updates by itself: pull the new image or binary
          when it suits you.
        </p>
        <div className="mt-3 flex flex-wrap gap-2">
          {page && (
            <a
              href={page}
              target="_blank"
              rel="noreferrer noopener"
              className="inline-flex h-8 items-center gap-1 rounded-full bg-primary px-3 text-xs font-bold text-primary-foreground transition-transform hover:scale-[1.03] active:scale-[0.97]"
            >
              What's new on github.com <ArrowUpRightIcon className="size-3.5" />
            </a>
          )}
          <a
            href={GUIDE}
            target="_blank"
            rel="noreferrer noopener"
            className="inline-flex h-8 items-center gap-1 rounded-full bg-secondary px-3 text-xs font-bold transition-transform hover:scale-[1.03] active:scale-[0.97]"
          >
            How to update, on github.com <ArrowUpRightIcon className="size-3.5" />
          </a>
        </div>
      </div>
    </motion.div>
  );
}
