import { RefreshCwIcon, SparklesIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { SPRING } from "@/components/motion";
import { updateLine } from "@/lib/compat";
import { FRESH, entryOf, look, type Freshness } from "@/lib/fresh";
import { useInstances } from "@/fuwa/hooks";
import { useI18n } from "@/i18n/react";
import { reduceMotion } from "@/lib/prefs";
import { reportError, reportUsage } from "@/lib/reports";

/** How often the page looks for a newer app. */
const EVERY_MS = 5 * 60 * 1000;
function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

function sameOrigin(url: string): boolean {
  try {
    return new URL(url).origin === location.origin;
  } catch {
    return false;
  }
}

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

/**
 * When the instance serving this page deploys a newer web app (fuwa.chat
 * does on every merge), a small pill says so with a Reload button. It also
 * says "Update fuwa to use …" when an instance has a feature this app is too
 * old for (`lib/compat.ts`). Nothing
 * is forced: the page never reloads by itself, and "Later" hides the pill
 * until the next deploy. Only the page's own instance is asked, the same
 * index.html it loaded from (`lib/fresh.ts` decides).
 */
export function UpdateReady() {
  /** The newer build this page is behind, once it's sure. */
  const [behind, setBehind] = useState<string | null>(null);
  /** The build "Later" was pressed for; a newer deploy asks again. */
  const [later, setLater] = useState<string | null>(null);
  const fresh = useRef<Freshness>(FRESH);
  const { t } = useI18n();
  // An instance with features this app doesn't know asks for a newer app too
  // (lib/compat.ts); the rest keeps working.
  // Reload only helps when it's this page's own instance; another instance's
  // features need the app it serves, or a newer fuwa there.
  const need =
    useInstances()
      .map((i) => ({ line: updateLine(i.node?.versions, i.node?.name || hostOf(i.url), t), own: sameOrigin(i.url) }))
      .find((n) => n.line !== null) ?? null;
  const needs = need?.line ?? null;
  const showing = behind && behind !== later ? "updated" : needs && needs !== later ? "needs" : null;
  const canReload = showing === "updated" || !!need?.own;

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
      const now = fresh.current;
      // A build counts once it's been seen twice in a row, a later deploy too.
      setBehind((prev) => (!now.stale ? null : now.streak >= 2 ? now.seen : prev));
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

  function reload() {
    reportUsage("web.update_reload");
    location.reload();
  }

  const calm = reduceMotion();
  return (
    <div className="pointer-events-none fixed inset-x-0 top-3 z-50 flex justify-center px-4">
      <AnimatePresence>
      {showing && (
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
          <span className="min-w-0 truncate font-semibold">{showing === "updated" ? t("shell.update.updated") : needs}</span>
          {canReload && (
            <button
              type="button"
              onClick={reload}
              className="flex h-8 shrink-0 items-center gap-1.5 rounded-full bg-primary px-3 text-xs font-bold text-primary-foreground transition-transform hover:scale-[1.03] active:scale-[0.97]"
            >
              <RefreshCwIcon className="size-3.5" />
              {t("shell.update.reload")}
            </button>
          )}
          <button
            type="button"
            onClick={() => setLater(showing === "updated" ? behind : needs)}
            aria-label={t("shell.update.later")}
            title={t("shell.update.laterHint")}
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
