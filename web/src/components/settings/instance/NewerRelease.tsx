import { ArrowUpRightIcon, GiftIcon } from "lucide-react";
import { m as motion } from "motion/react";
import type { Node } from "@/gen/fuwa/v1/types_pb";
import { SPRING } from "@/lib/motion";
import { type I18n, useI18n } from "@/i18n/react";
import { formatStamp, toDate } from "@/lib/format";
import { reduceMotion } from "@/lib/prefs";

const GUIDE = "https://github.com/waifu-devs/fuwa/blob/master/docs/self-hosting.md#updating";

/** A stamp that starts with "Today" or "Yesterday", lowercased to sit mid-sentence. */
function midSentence(t: I18n["t"], stamp: string) {
  const day = [t("common.time.today"), t("common.time.yesterday")].find((d) => stamp.startsWith(d));
  return day ? day.toLowerCase() + stamp.slice(day.length) : stamp;
}

/**
 * "fuwa 0.4.2 is out": shown to the instance's admins when its daily check
 * (server/src/releases.rs) found a newer release. Nothing updates by itself;
 * the links say where to read about it and how. Both open github.com, and say so.
 */
export function NewerRelease({ node }: { node: Node | null | undefined }) {
  const { t } = useI18n();
  const release = node?.versions?.newerRelease;
  if (!release?.version) return null;
  const calm = reduceMotion();
  const stamp = release.publishedAt ? formatStamp(toDate(release.publishedAt)) : null;
  // Mid-sentence: "came out today at 1:00 AM".
  const when = stamp && midSentence(t, stamp);
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
        <p className="font-bold">{t("instancesettings.release.out", { version: release.version })}</p>
        <p className="mt-0.5 text-sm text-muted-foreground">
          {when
            ? t("instancesettings.release.runsCameOut", { current: node?.version ?? "", version: release.version, when })
            : t("instancesettings.release.runs", { current: node?.version ?? "" })}
        </p>
        <div className="mt-3 flex flex-wrap gap-2">
          {page && (
            <a
              href={page}
              target="_blank"
              rel="noreferrer noopener"
              className="inline-flex h-8 items-center gap-1 rounded-full bg-primary px-3 text-xs font-bold text-primary-foreground transition-transform hover:scale-[1.03] active:scale-[0.97]"
            >
              {t("instancesettings.release.whatsNew")} <ArrowUpRightIcon className="size-3.5" />
            </a>
          )}
          <a
            href={GUIDE}
            target="_blank"
            rel="noreferrer noopener"
            className="inline-flex h-8 items-center gap-1 rounded-full bg-secondary px-3 text-xs font-bold transition-transform hover:scale-[1.03] active:scale-[0.97]"
          >
            {t("instancesettings.release.howTo")} <ArrowUpRightIcon className="size-3.5" />
          </a>
        </div>
      </div>
    </motion.div>
  );
}
