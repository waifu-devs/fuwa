import { EyeOffIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useRef, useState, type MouseEvent, type ReactNode } from "react";
import { useI18n } from "@/i18n/react";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";

/** The "Spoiler" pill, over a covered file and on one marked to send as one. */
export function SpoilerBadge({ className }: { className?: string }) {
  const { t } = useI18n();
  return (
    <span className={cn("inline-flex items-center gap-1 rounded-full bg-black/65 px-2.5 py-1 text-[0.7rem] font-extrabold tracking-wide text-white uppercase shadow-sm backdrop-blur-sm", className)}>
      <EyeOffIcon className="size-3.5" />
      {t("chat.files.spoilerBadge")}
    </span>
  );
}

/**
 * A file marked as a spoiler: covered until the reader clicks it or presses
 * Enter or Space on it, then shown for as long as it's on screen, like a
 * spoiler in text. Pictures and videos (`media`) are blurred past telling
 * what they are; a file's card (`card`) just enough to hide its name. While
 * covered, nothing inside can be focused or clicked, so the first click only
 * uncovers it: it never opens the viewer or starts a download. The blur
 * clears with a short fade (styles/app.css, `.spoiler-file`), at once with
 * reduced motion. Anything not marked is shown as it is.
 */
export function SpoilerCover({
  spoiler,
  look,
  animate = false,
  className,
  onShown,
  children,
}: {
  spoiler: boolean;
  look: "media" | "card";
  /** Whether the pill springs in (a message arriving), or is simply there. */
  animate?: boolean;
  /** Its size and corners, the same as what it covers. */
  className?: string;
  /** Told once it's uncovered (the picture viewer steps only through what's been shown). */
  onShown?: () => void;
  children: ReactNode;
}) {
  const { t } = useI18n();
  const [shown, setShown] = useState(false);
  const inside = useRef<HTMLDivElement>(null);
  if (!spoiler) return children;
  const reveal = (event: MouseEvent) => {
    // Inside a clickable row (a search result, a message), it uncovers and does nothing else.
    event.preventDefault();
    event.stopPropagation();
    setShown(true);
    onShown?.();
    // From the keyboard, focus goes on to what was covered (its player, its download) rather than getting lost.
    if (event.detail === 0) {
      requestAnimationFrame(() => inside.current?.querySelector<HTMLElement>("button, a[href], video, audio, [tabindex]")?.focus());
    }
  };
  return (
    <div className={cn("spoiler-file relative max-w-full", className)} data-look={look} data-hidden={shown ? undefined : ""}>
      <div ref={inside} className="spoiler-file-inside" inert={!shown}>
        {children}
      </div>
      <AnimatePresence>
        {!shown && (
          <motion.button
            key="cover"
            type="button"
            aria-label={t("chat.attachments.spoiler")}
            title={t("chat.attachments.spoiler")}
            onClick={reveal}
            whileHover="hover"
            whileTap="tap"
            exit={{ opacity: 0 }}
            transition={{ duration: 0.2 }}
            className="absolute inset-0 z-10 grid cursor-pointer place-items-center rounded-[inherit] bg-foreground/5 transition-colors outline-none hover:bg-foreground/10 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
          >
            <motion.span
              initial={animate ? { opacity: 0, scale: 0.6 } : false}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.8 }}
              variants={{ hover: { scale: 1.06 }, tap: { scale: 0.92 } }}
              transition={SPRING}
            >
              <SpoilerBadge />
            </motion.span>
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}
