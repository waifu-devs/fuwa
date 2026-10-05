import { XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState } from "react";
import { AnnouncementTone, type Announcement } from "@/gen/fuwa/v1/types_pb";
import { useFuwa } from "@/fuwa/store";
import { InlineMarkdown } from "@/components/Markdown";
import { useI18n } from "@/i18n/react";
import { endsLabel, isLive, TONES, toneOf } from "@/lib/announcement";
import { formatStamp, toDate } from "@/lib/format";
import { SLIDE_IN } from "@/lib/motion";
import { useNow } from "@/lib/notifications";
import { cn } from "@/lib/utils";

/**
 * The instance's announcement, across the top of the app while you're on
 * that instance: news in the theme's color, heads-ups in amber, and urgent
 * ones in red, which can't be closed. A closed banner stays closed on this
 * device until the admins put up a new one.
 */

const DISMISSED = "fuwa:announcement-closed:";

function closedId(instanceKey: string) {
  try {
    return localStorage.getItem(DISMISSED + instanceKey);
  } catch {
    return null;
  }
}

export function AnnouncementBanner({ instanceKey }: { instanceKey: string | undefined }) {
  const announcement = useFuwa((s) => (instanceKey ? s.instances[instanceKey]?.node?.announcement : undefined));
  const now = useNow(15_000);
  const [closed, setClosed] = useState<Record<string, string | null>>({});
  const a = announcement;
  const key = instanceKey ?? "";
  const closedHere = key in closed ? closed[key] : closedId(key);
  const show = !!instanceKey && isLive(a, now) && (toneOf(a) === AnnouncementTone.CRITICAL || closedHere !== a.id);

  function close() {
    if (!a) return;
    setClosed((c) => ({ ...c, [key]: a.id }));
    try {
      localStorage.setItem(DISMISSED + key, a.id);
    } catch {
      // Closed until the page reloads.
    }
  }

  return (
    <AnimatePresence mode="popLayout" initial={false}>
      {show && (
        <motion.div
          key={key}
          {...SLIDE_IN}
          transition={{ type: "spring", stiffness: 420, damping: 40 }}
          className="shrink-0"
        >
          <BannerBody announcement={a} onClose={toneOf(a) === AnnouncementTone.CRITICAL ? undefined : close} />
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/** The banner itself, also used as the live preview in the Announcement settings. */
export function BannerBody({ announcement: a, onClose, preview = false }: { announcement: Announcement; onClose?: () => void; preview?: boolean }) {
  const { t } = useI18n();
  const tone = TONES[toneOf(a)];
  const Icon = tone.icon;
  const critical = toneOf(a) === AnnouncementTone.CRITICAL;
  return (
    <div role={preview ? undefined : critical ? "alert" : "status"} className={cn("announcement relative flex items-center gap-3 px-3 py-2 text-sm sm:justify-center sm:px-12", tone.className)}>
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={`${toneOf(a)}`}
          initial={{ scale: 0, rotate: -40 }}
          animate={{ scale: 1, rotate: 0 }}
          exit={{ scale: 0, rotate: 40 }}
          transition={{ type: "spring", stiffness: 520, damping: 14, delay: preview ? 0 : 0.12 }}
          className="announcement-icon relative grid size-7 shrink-0 place-items-center rounded-full"
        >
          {critical && <span className="absolute inset-0 animate-ping rounded-full bg-white/40 motion-reduce:hidden" />}
          <Icon className="announcement-glyph relative size-4" />
        </motion.span>
      </AnimatePresence>
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.p
          key={a.id || a.text}
          initial={{ opacity: 0, y: -8, filter: "blur(4px)" }}
          animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
          exit={{ opacity: 0, y: 8, filter: "blur(4px)" }}
          transition={{ type: "spring", stiffness: 420, damping: 34, delay: preview ? 0 : 0.08 }}
          className="line-clamp-2 min-w-0 flex-1 font-bold sm:flex-initial"
        >
          <InlineMarkdown>{a.text}</InlineMarkdown>
        </motion.p>
      </AnimatePresence>
      {a.endsAt && (
        <span className="hidden shrink-0 rounded-full bg-black/10 px-2 py-0.5 text-xs font-bold whitespace-nowrap sm:inline dark:bg-white/10" title={formatStamp(toDate(a.endsAt))}>
          {t("shell.announcement.until", { time: endsLabel(toDate(a.endsAt)) })}
        </span>
      )}
      {onClose && (
        <button
          type="button"
          onClick={onClose}
          aria-label={t("shell.announcement.close")}
          className="group grid size-7 shrink-0 place-items-center rounded-full transition hover:bg-black/10 active:scale-90 sm:absolute sm:top-1/2 sm:right-2 sm:-translate-y-1/2 dark:hover:bg-white/15"
        >
          <XIcon className="size-4 transition-transform duration-300 group-hover:rotate-90" />
        </button>
      )}
    </div>
  );
}
