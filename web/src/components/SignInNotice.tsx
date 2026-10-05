import { timestampDate } from "@bufbuild/protobuf/wkt";
import { ShieldAlertIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState } from "react";
import { useFuwa } from "@/fuwa/store";
import { ProviderMark } from "@/components/ProviderMarks";
import { useI18n } from "@/i18n/react";
import { openSettings } from "@/lib/ui";

const CLOSED = "fuwa:sign-in-notice-closed:";

function closedOn(instanceKey: string) {
  try {
    return localStorage.getItem(CLOSED + instanceKey);
  } catch {
    return null;
  }
}

/**
 * Says so on every device when a way to sign in (Google, X, Twitch) was
 * added to your account in the last week, so one you didn't add gets seen
 * and undone. Closed, it stays closed on this device until another is added.
 */
export function SignInNotice({ instanceKey }: { instanceKey: string | undefined }) {
  const { t } = useI18n();
  const latest = useFuwa((s) => (instanceKey ? s.instances[instanceKey]?.recentSignIns?.[0] : undefined));
  const [closed, setClosed] = useState<Record<string, string | null>>({});
  const key = instanceKey ?? "";
  const id = latest?.linkedAt ? `${latest.kind}:${latest.linkedAt.seconds}` : "";
  const closedHere = key in closed ? closed[key] : closedOn(key);
  const show = !!instanceKey && !!latest && closedHere !== id;

  function close() {
    setClosed((c) => ({ ...c, [key]: id }));
    try {
      localStorage.setItem(CLOSED + key, id);
    } catch {
      // Closed until the page reloads.
    }
  }

  return (
    <AnimatePresence initial={false}>
      {show && latest && (
        <motion.div
          key={id}
          initial={{ opacity: 0, y: -12 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -12 }}
          transition={{ type: "spring", stiffness: 420, damping: 40 }}
          className="shrink-0"
          data-testid="sign-in-notice"
        >
          <div role="status" className="announcement announcement-warning relative flex items-center gap-3 px-3 py-2 text-sm sm:justify-center sm:px-12">
            <motion.span
              initial={{ scale: 0, rotate: -40 }}
              animate={{ scale: 1, rotate: 0 }}
              transition={{ type: "spring", stiffness: 520, damping: 14, delay: 0.12 }}
              className="announcement-icon relative grid size-7 shrink-0 place-items-center rounded-full"
            >
              <ProviderMark id={latest.kind} className="announcement-glyph size-3.5" />
            </motion.span>
            <p className="min-w-0 flex-1 font-bold sm:flex-initial">
              {t("shell.signInNotice.added", {
                name: latest.name,
                date: timestampDate(latest.linkedAt!).toLocaleDateString(undefined, { month: "short", day: "numeric" }),
              })}
            </p>
            <button
              type="button"
              onClick={() => openSettings("sign-in")}
              className="inline-flex shrink-0 items-center gap-1 rounded-full bg-black/10 px-2.5 py-0.5 text-xs font-bold transition hover:bg-black/15 dark:bg-white/10 dark:hover:bg-white/15"
            >
              <ShieldAlertIcon className="size-3.5" /> {t("shell.signInNotice.review")}
            </button>
            <button
              type="button"
              onClick={close}
              aria-label={t("shell.announcement.close")}
              className="group grid size-7 shrink-0 place-items-center rounded-full transition hover:bg-black/10 active:scale-90 sm:absolute sm:top-1/2 sm:right-2 sm:-translate-y-1/2 dark:hover:bg-white/15"
            >
              <XIcon className="size-4 transition-transform duration-300 group-hover:rotate-90" />
            </button>
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
