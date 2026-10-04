import { RefreshCwIcon, SparklesIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { getCalls } from "@/calls/state";
import { SPRING } from "@/components/motion";
import { FRESH, entryOf, look, mayReload, type Freshness } from "@/lib/fresh";
import { reduceMotion } from "@/lib/prefs";
import { reportError, reportUsage } from "@/lib/reports";

/** How often the page looks for a newer app. */
const EVERY_MS = 5 * 60 * 1000;
/** How often a page that's behind sees whether it may reload by itself. */
const TRY_EVERY_MS = 30 * 1000;

/** This page's own entry script, as a path. */
function ownEntry(): string | null {
  const src = document.querySelector<HTMLScriptElement>('script[type="module"][src]')?.getAttribute("src");
  return src ? new URL(src, location.href).pathname : null;
}

/** The entry script of the app the instance serves now, or null when it can't say. */
async function servedEntry(): Promise<string | null> {
  try {
    // Revalidated each time (the instance answers 304 while nothing changed); no cookies go with it.
    const response = await fetch("/", { cache: "no-cache", credentials: "omit", headers: { accept: "text/html" } });
    if (!response.ok || !response.headers.get("content-type")?.includes("text/html")) return null;
    const src = entryOf(await response.text());
    if (!src) reportError("web_update_check_unreadable", "UpdateReady");
    return src ? new URL(src, location.href).pathname : null;
  } catch {
    // Offline or restarting: try again next time.
    return null;
  }
}

/** Text typed and not sent anywhere on the page. */
function hasDraft(): boolean {
  for (const el of document.querySelectorAll<HTMLTextAreaElement | HTMLInputElement>("textarea, input[type=text], input:not([type])")) {
    if (el.value.trim()) return true;
  }
  return [...document.querySelectorAll<HTMLElement>("[contenteditable=true]")].some((el) => el.textContent?.trim());
}

/**
 * When the instance serving this page deploys a newer web app (fuwa.chat
 * does on every merge), a small pill says so with a Reload button. The page
 * also reloads by itself once it's in the background or nobody has touched
 * it for a while, but never with a message half typed, in a call, or with a
 * dialog open. Only the page's own instance is asked, the same index.html
 * it loaded from (`lib/fresh.ts` decides).
 */
export function UpdateReady() {
  const [stale, setStale] = useState(false);
  const [later, setLater] = useState(false);
  const fresh = useRef<Freshness>(FRESH);
  const lastInput = useRef(Date.now());

  useEffect(() => {
    // The dev server swaps code in place; only a built app looks.
    if (import.meta.env.DEV) return;
    const ours = ownEntry();
    if (!ours) return;
    let stopped = false;
    async function check() {
      const found = await servedEntry();
      if (stopped) return;
      fresh.current = look(fresh.current, ours!, found);
      setStale(fresh.current.stale);
    }
    const timer = setInterval(check, EVERY_MS);
    const onVisible = () => document.visibilityState === "visible" && void check();
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("online", check);
    return () => {
      stopped = true;
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("online", check);
    };
  }, []);

  useEffect(() => {
    if (!stale) return;
    const touched = () => (lastInput.current = Date.now());
    const tryReload = () => {
      const moment = {
        hidden: document.visibilityState === "hidden",
        idleMs: Date.now() - lastInput.current,
        draft: hasDraft(),
        inCall: !!getCalls().call,
        dialog: !!document.querySelector('[role="dialog"], [role="alertdialog"]'),
      };
      if (mayReload(moment)) location.reload();
    };
    window.addEventListener("pointerdown", touched, { passive: true });
    window.addEventListener("keydown", touched, { passive: true });
    document.addEventListener("visibilitychange", tryReload);
    const timer = setInterval(tryReload, TRY_EVERY_MS);
    return () => {
      window.removeEventListener("pointerdown", touched);
      window.removeEventListener("keydown", touched);
      document.removeEventListener("visibilitychange", tryReload);
      clearInterval(timer);
    };
  }, [stale]);

  function reload() {
    reportUsage("web.update_reload");
    location.reload();
  }

  const calm = reduceMotion();
  return (
    <div className="pointer-events-none fixed inset-x-0 top-3 z-50 flex justify-center px-4">
      <AnimatePresence>
      {stale && !later && (
        <motion.div
          key="update-ready"
          role="status"
          initial={calm ? { opacity: 0 } : { opacity: 0, y: -24, scale: 0.96 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={calm ? { opacity: 0 } : { opacity: 0, y: -16, scale: 0.98 }}
          transition={SPRING}
          className="pointer-events-auto flex max-w-full items-center gap-2.5 rounded-full border bg-popover/95 py-1.5 pr-1.5 pl-2 text-sm text-popover-foreground shadow-lg backdrop-blur"
        >
          <span className="relative grid size-7 shrink-0 place-items-center rounded-full bg-primary/15 text-primary">
            {!calm && (
              <motion.span
                aria-hidden
                className="absolute inset-0 rounded-full bg-primary/30"
                initial={{ opacity: 0.6, scale: 1 }}
                animate={{ opacity: 0, scale: 1.7 }}
                transition={{ duration: 1.6, repeat: 2, ease: "easeOut" }}
              />
            )}
            <SparklesIcon className="size-4" />
          </span>
          <span className="min-w-0 truncate font-semibold">fuwa was updated</span>
          <button
            type="button"
            onClick={reload}
            className="flex h-8 shrink-0 items-center gap-1.5 rounded-full bg-primary px-3 text-xs font-bold text-primary-foreground transition-transform hover:scale-[1.03] active:scale-[0.97]"
          >
            <RefreshCwIcon className="size-3.5" />
            Reload
          </button>
          <button
            type="button"
            onClick={() => setLater(true)}
            aria-label="Later"
            title="Later: it reloads by itself when you're away"
            className="grid size-8 shrink-0 place-items-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          >
            <XIcon className="size-4" />
          </button>
        </motion.div>
      )}
      </AnimatePresence>
    </div>
  );
}
