import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { layoutLines, linesIn } from "@/components/virtual-range";

/** A line of a virtual list that's drawn: its key, its place in the list and its top. */
export type VirtualLine = { key: string; index: number; top: number };

/** The nearest box around `el` that scrolls up and down. */
function scrollParent(el: HTMLElement): HTMLElement {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const overflow = getComputedStyle(p).overflowY;
    if (overflow === "auto" || overflow === "scroll") return p;
  }
  return (document.scrollingElement as HTMLElement | null) ?? document.documentElement;
}

/**
 * A list that draws only the lines in view, in whatever scrolls around it
 * (a settings page, a sidebar), for lines whose heights differ. Each drawn
 * line is measured (`measure` as its ref, with `data-vkey` set to its key)
 * and remembered by key, so it keeps its height when it scrolls away and
 * back; until then it counts as `estimate`. Lines sit absolutely at their
 * `top` (a transform, so a `.member-line` eases when the order changes).
 *
 * When a line above the view turns out taller or shorter than thought, the
 * scroll moves by as much, so what's on screen stays put, and the list is
 * marked `data-settling` for a frame so lines jump instead of gliding.
 */
export function useVirtualList(keys: readonly string[], { estimate, gap = 0, overscan = 480 }: { estimate: number; gap?: number; overscan?: number }) {
  const listRef = useRef<HTMLElement | null>(null);
  const scrollerRef = useRef<HTMLElement | null>(null);
  const sizes = useRef(new Map<string, number>());
  const [version, setVersion] = useState(0);
  // The stretch of the list in view, in the list's own coordinates.
  const [view, setView] = useState(() => ({ top: 0, height: typeof window === "undefined" ? 800 : window.innerHeight }));

  const layout = useMemo(() => {
    void version;
    const heights = keys.map((k) => sizes.current.get(k) ?? estimate);
    return { heights, ...layoutLines(heights, gap) };
  }, [keys, version, estimate, gap]);
  const layoutRef = useRef(layout);
  layoutRef.current = layout;
  const indexOf = useMemo(() => new Map(keys.map((k, n) => [k, n])), [keys]);
  const indexRef = useRef(indexOf);
  indexRef.current = indexOf;

  // Follows the scroll, at most once a frame.
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const scroller = scrollParent(list);
    scrollerRef.current = scroller;
    let frame = 0;
    const read = () => {
      frame = 0;
      const top = Math.round(scroller.getBoundingClientRect().top - list.getBoundingClientRect().top);
      const height = scroller.clientHeight;
      setView((v) => (v.top === top && v.height === height ? v : { top, height }));
    };
    const soon = () => {
      if (!frame) frame = requestAnimationFrame(read);
    };
    read();
    const target: HTMLElement | Window = scroller === document.scrollingElement ? window : scroller;
    target.addEventListener("scroll", soon, { passive: true });
    const ro = new ResizeObserver(soon);
    ro.observe(scroller);
    // The list moving within it (something above it growing) changes what part is in view too.
    ro.observe(list);
    return () => {
      target.removeEventListener("scroll", soon);
      ro.disconnect();
      if (frame) cancelAnimationFrame(frame);
    };
  }, []);

  // Measures drawn lines; one observer for all of them.
  const [observer] = useState(() =>
    typeof ResizeObserver === "undefined"
      ? null
      : new ResizeObserver((entries) => {
          let changed = false;
          let shift = 0;
          const { tops } = layoutRef.current;
          const scroller = scrollerRef.current;
          const viewTop = scroller && listRef.current ? scroller.getBoundingClientRect().top - listRef.current.getBoundingClientRect().top : 0;
          for (const entry of entries) {
            const el = entry.target as HTMLElement;
            const key = el.dataset.vkey;
            if (!key || !el.isConnected) continue;
            const h = Math.round(entry.borderBoxSize?.[0]?.blockSize ?? el.offsetHeight);
            if (h <= 0) continue;
            const before = sizes.current.get(key) ?? estimate;
            if (before === h) {
              sizes.current.set(key, h);
              continue;
            }
            sizes.current.set(key, h);
            changed = true;
            const n = indexRef.current.get(key);
            if (n !== undefined && tops[n]! + before <= viewTop) shift += h - before;
          }
          if (!changed) return;
          const list = listRef.current;
          if (list) {
            list.dataset.settling = "";
            requestAnimationFrame(() => requestAnimationFrame(() => delete list.dataset.settling));
          }
          if (shift && scroller) scroller.scrollTop += shift;
          setVersion((v) => v + 1);
        }),
  );
  const measure = useCallback(
    (el: HTMLElement | null) => {
      if (!el || !observer) return;
      observer.observe(el);
      return () => observer.unobserve(el);
    },
    [observer],
  );

  const [start, end] = linesIn(layout.tops, layout.heights, view.top - overscan, view.top + view.height + overscan);
  const lines: VirtualLine[] = [];
  for (let n = start; n < end; n++) lines.push({ key: keys[n]!, index: n, top: layout.tops[n]! });
  return { listRef, measure, total: layout.total, lines };
}
