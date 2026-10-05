import * as Popover from "@radix-ui/react-popover";
import { ArrowUpRightIcon, LockIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState } from "react";
import { Private } from "@/components/Private";
import { SPRING } from "@/components/motion";
import { T, useI18n } from "@/i18n/react";
import { hostedByUs } from "@/lib/hosted";
import { cn } from "@/lib/utils";

/** A five-petal flower, the badge's mark. Its petals open one after another when it appears. */
export function FlowerMark({ className, bloom = true }: { className?: string; bloom?: boolean }) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden className={cn("shrink-0", className)}>
      {[0, 1, 2, 3, 4].map((i) => (
        <g key={i} transform={`rotate(${i * 72} 12 12)`}>
          <motion.ellipse
            cx="12"
            cy="6.6"
            rx="3.7"
            ry="5.2"
            fill="currentColor"
            fillOpacity={0.9}
            style={{ transformOrigin: "50% 100%" }}
            initial={bloom ? { scale: 0 } : false}
            animate={{ scale: 1 }}
            transition={{ ...SPRING, stiffness: 380, damping: 18, delay: 0.08 + i * 0.06 }}
          />
        </g>
      ))}
      <motion.circle
        cx="12"
        cy="12"
        r="2.4"
        className="fill-background"
        initial={bloom ? { scale: 0 } : false}
        animate={{ scale: 1 }}
        transition={{ ...SPRING, delay: 0.45 }}
      />
    </svg>
  );
}

/**
 * "Hosted by Waifu Devs", for instances the app reached at one of our own
 * addresses (see lib/hosted). Nothing shows for any other instance. Click it
 * for what it means. `mark` is just the flower, for tight spots; `still` is a
 * plain label for inside something already clickable.
 */
export function HostedBadge({ url, variant = "chip", className }: { url: string | undefined; variant?: "chip" | "mark" | "still"; className?: string }) {
  const [open, setOpen] = useState(false);
  const { t } = useI18n();
  if (!hostedByUs(url)) return null;
  const label = t("shell.hosted.label");
  const host = new URL(url!).host;

  if (variant === "still") {
    return (
      <span className={cn("hosted-chip", className)}>
        <FlowerMark className="size-3.5" />
        {label}
      </span>
    );
  }

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        {variant === "mark" ? (
          <button
            type="button"
            title={label}
            className={cn("group inline-grid size-5 shrink-0 place-items-center rounded-full text-primary transition hover:bg-primary/15 active:scale-90", className)}
          >
            <FlowerMark className="size-3.5 transition-transform duration-700 ease-out group-hover:rotate-[72deg]" />
            <span className="sr-only">{label}</span>
          </button>
        ) : (
          <button type="button" className={cn("hosted-chip group cursor-pointer transition active:scale-95", className)}>
            <FlowerMark className="size-3.5 transition-transform duration-700 ease-out group-hover:rotate-[72deg]" />
            {label}
          </button>
        )}
      </Popover.Trigger>
      <AnimatePresence>
        {open && (
          <Popover.Portal forceMount>
            <Popover.Content forceMount side="bottom" align="start" sideOffset={8} collisionPadding={12} className="z-50 outline-none">
              <motion.div
                initial={{ opacity: 0, scale: 0.9, y: -6 }}
                animate={{ opacity: 1, scale: 1, y: 0 }}
                exit={{ opacity: 0, scale: 0.95, y: -4 }}
                transition={SPRING}
                style={{ transformOrigin: "var(--radix-popover-content-transform-origin)" }}
                className="relative w-[min(20rem,calc(100vw-24px))] overflow-hidden rounded-2xl border bg-popover p-4 text-popover-foreground shadow-xl"
              >
                <div aria-hidden className="absolute -top-16 -right-12 size-40 rounded-full bg-primary/20 blur-2xl" />
                <div className="relative flex items-center gap-3">
                  <span className="grid size-11 shrink-0 place-items-center rounded-2xl bg-primary/15 text-primary">
                    <FlowerMark className="size-7" />
                  </span>
                  <div className="min-w-0">
                    <p className="font-extrabold">{label}</p>
                    <p className="text-xs text-muted-foreground">{t("shell.hosted.who")}</p>
                  </div>
                </div>
                <motion.p
                  initial={{ opacity: 0, y: 4 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ ...SPRING, delay: 0.15 }}
                  className="relative mt-3 flex items-center gap-2 rounded-xl bg-muted/60 px-3 py-2 text-xs"
                >
                  <LockIcon className="size-3.5 shrink-0 text-emerald-600 dark:text-emerald-400" />
                  <span className="min-w-0">
                    <T k="shell.hosted.checked" values={{ host: <Private text={host} className="font-mono font-bold" /> }} />
                  </span>
                </motion.p>
                <motion.p
                  initial={{ opacity: 0, y: 4 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ ...SPRING, delay: 0.22 }}
                  className="relative mt-3 text-xs text-muted-foreground"
                >
                  {t("shell.hosted.why")}
                </motion.p>
                <motion.a
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  transition={{ delay: 0.3 }}
                  href="https://www.waifu.dev/projects"
                  target="_blank"
                  rel="noreferrer"
                  className="group relative mt-3 inline-flex items-center gap-1 text-xs font-bold text-primary hover:underline"
                >
                  {t("shell.hosted.about")}
                  <ArrowUpRightIcon className="size-3.5 transition group-hover:translate-x-0.5 group-hover:-translate-y-0.5" />
                </motion.a>
              </motion.div>
            </Popover.Content>
          </Popover.Portal>
        )}
      </AnimatePresence>
    </Popover.Root>
  );
}
