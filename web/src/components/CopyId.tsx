import { CheckIcon, FingerprintIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { SPRING } from "@/components/motion";
import { usePrefs } from "@/lib/prefs";
import { copy } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Developer mode's Copy ID button; it only exists while developer mode is on. */
export function CopyId({ id, what, className }: { id: string; what: string; className?: string }) {
  const on = usePrefs((p) => p.developerMode);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1200);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <AnimatePresence initial={false}>
      {on && (
        <motion.button
          type="button"
          initial={{ opacity: 0, scale: 0.5 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.5 }}
          transition={SPRING}
          whileTap={{ scale: 0.85 }}
          onClick={(e) => {
            e.stopPropagation();
            copy(id, what);
            setCopied(true);
          }}
          aria-label={`Copy ${what}`}
          title={`Copy ${what}`}
          className={cn("grid size-8 shrink-0 place-items-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground", className)}
        >
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.span key={String(copied)} initial={{ scale: 0.4, rotate: -30 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0.4, opacity: 0 }} transition={SPRING}>
              {copied ? <CheckIcon className="size-4 text-emerald-500" /> : <FingerprintIcon className="size-4" />}
            </motion.span>
          </AnimatePresence>
        </motion.button>
      )}
    </AnimatePresence>
  );
}
