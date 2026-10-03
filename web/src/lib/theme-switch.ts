import { flushSync } from "react-dom";
import { reduceMotion, setPrefs, type Prefs } from "@/lib/prefs";

/**
 * Changes the theme with a flourish: the new one spreads out in a circle from
 * where you clicked (a view transition of the whole page), where the browser
 * can. Elsewhere, and with reduced motion, the colors cross-fade as before
 * (the tokens are registered properties, styles/app.css).
 */
export function switchTheme(patch: Partial<Prefs>, from?: { x: number; y: number }) {
  const doc = document as Document & { startViewTransition?: (update: () => void) => { ready: Promise<void>; finished: Promise<void> } };
  if (!doc.startViewTransition || reduceMotion()) {
    setPrefs(patch);
    return;
  }
  const root = document.documentElement;
  const x = from?.x ?? innerWidth / 2;
  const y = from?.y ?? innerHeight / 2;
  const radius = Math.hypot(Math.max(x, innerWidth - x), Math.max(y, innerHeight - y));
  // The snapshot must show the new colors straight away, not the start of their cross-fade.
  root.dataset.themeSwitching = "";
  const transition = doc.startViewTransition(() => flushSync(() => setPrefs(patch)));
  transition.ready
    .then(() =>
      root.animate(
        { clipPath: [`circle(0px at ${x}px ${y}px)`, `circle(${radius}px at ${x}px ${y}px)`] },
        { duration: 700, easing: "cubic-bezier(0.22, 1, 0.36, 1)", pseudoElement: "::view-transition-new(root)" },
      ),
    )
    .catch(() => {});
  transition.finished.finally(() => delete root.dataset.themeSwitching).catch(() => {});
}

/** Where a click happened, to spread the theme from. */
export const clickPoint = (e: { clientX: number; clientY: number }) =>
  e.clientX || e.clientY ? { x: e.clientX, y: e.clientY } : undefined;
