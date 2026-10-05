import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import { move, sameLayout, type Drop, type Layout } from "@/lib/arrange";
import { reduceMotion } from "@/lib/prefs";

/**
 * Dragging channels and categories into place, in a list that marks its rows:
 *
 * - a channel row: `data-arrange="channel"`, `data-id`, `data-parent` (its category, or "")
 * - a category's header: `data-arrange="category"`, `data-id`, and `data-collapsed` when its channels are hidden
 *
 * in display order, inside `container` (the element that scrolls, positioned).
 * Press a row and move (or hold it a moment, on a touch screen) to pick it up.
 * A channel goes between any two channels, into a category through its header
 * (or above every category, out of any), and a category between the others.
 *
 * Everything during a drag happens outside React: the picked-up copy follows
 * the pointer through `transform`, and the drop line moves only when the place
 * it marks changes, so the list itself never renders while you drag. React
 * hears about it once, with the new layout, when you let go.
 */
export function useArrange({
  container,
  enabled,
  layout,
  onArrange,
  handle,
}: {
  container: RefObject<HTMLElement | null>;
  /** Whether you may rearrange (Manage Channels); without it rows are only clicked. */
  enabled: boolean;
  layout: Layout;
  onArrange: (next: Layout, moved: string) => void;
  /** A selector for the part of a row that picks it up; the whole row by default. */
  handle?: string;
}) {
  const latest = useRef({ layout, onArrange });
  // Kept fresh after each commit, before any pointer event can read it.
  useLayoutEffect(() => {
    latest.current = { layout, onArrange };
  });

  useEffect(() => {
    const root = container.current;
    if (!root || !enabled) return;
    let drag: Dragging | null = null;
    let pending: Pending | null = null;

    const cancelPending = () => {
      if (!pending) return;
      clearTimeout(pending.timer);
      pending.row.removeAttribute("data-pressing");
      pending = null;
    };

    const onPointerDown = (e: PointerEvent) => {
      if (drag || pending || e.button !== 0 || !e.isPrimary) return;
      const target = e.target as Element;
      if (handle && !target.closest(handle)) return;
      const row = target.closest<HTMLElement>("[data-arrange]");
      if (!row || !root.contains(row) || target.closest("[data-arrange-skip]")) return;
      const touch = e.pointerType !== "mouse";
      // A handle is only for dragging: no text selection starting from it.
      if (handle) e.preventDefault();
      pending = {
        row,
        x: e.clientX,
        y: e.clientY,
        pointer: e.pointerId,
        // Touch waits for a hold, so a swipe still scrolls the list.
        timer: touch && !handle ? window.setTimeout(() => begin(), 320) : 0,
      };
      if (touch && !handle) row.setAttribute("data-pressing", "");
      row.toggleAttribute("data-touch", touch);
    };

    const begin = () => {
      if (!pending) return;
      const { row, x, y } = pending;
      cancelPending();
      drag = start(root, row, x, y, latest.current.layout);
      navigator.vibrate?.(8);
    };

    const onPointerMove = (e: PointerEvent) => {
      if (pending && e.pointerId === pending.pointer) {
        const far = Math.hypot(e.clientX - pending.x, e.clientY - pending.y);
        if (pending.timer) {
          if (far > 8) cancelPending();
        } else if (far > 5) {
          begin();
        }
      }
      if (drag) {
        drag.pointer = { x: e.clientX, y: e.clientY };
        e.preventDefault();
      }
    };

    const finish = (commit: boolean) => {
      cancelPending();
      if (!drag) return;
      const d = drag;
      drag = null;
      const next = commit && d.drop ? move(latest.current.layout, d.drop) : null;
      const changed = next && !sameLayout(next, latest.current.layout);
      if (changed) latest.current.onArrange(next, d.id);
      d.land(root, !!changed);
      // The click that ends a mouse drag mustn't open the channel.
      const swallow = (ev: Event) => {
        ev.preventDefault();
        ev.stopPropagation();
      };
      window.addEventListener("click", swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
    };

    const onPointerUp = () => finish(true);
    const onCancel = () => finish(false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && drag) {
        // Puts the row back, without also closing the dialog the list may sit in.
        e.preventDefault();
        e.stopPropagation();
        finish(false);
      }
    };
    // Links drag their address by default; rows here move instead.
    const onNativeDrag = (e: DragEvent) => {
      if ((e.target as Element).closest?.("[data-arrange]")) e.preventDefault();
    };
    // Held on a touch screen: no page scroll, no link menu.
    const onTouchMove = (e: TouchEvent) => {
      if (drag) e.preventDefault();
    };
    const onContextMenu = (e: Event) => {
      if (drag || pending?.timer) e.preventDefault();
    };

    root.addEventListener("pointerdown", onPointerDown);
    root.addEventListener("dragstart", onNativeDrag);
    root.addEventListener("contextmenu", onContextMenu);
    window.addEventListener("pointermove", onPointerMove, { passive: false });
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("touchmove", onTouchMove, { passive: false });
    window.addEventListener("blur", onCancel);
    return () => {
      finish(false);
      root.removeEventListener("pointerdown", onPointerDown);
      root.removeEventListener("dragstart", onNativeDrag);
      root.removeEventListener("contextmenu", onContextMenu);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("touchmove", onTouchMove);
      window.removeEventListener("blur", onCancel);
    };
  }, [container, enabled, handle]);
}

type Pending = { row: HTMLElement; x: number; y: number; pointer: number; timer: number };

/** A row as it was when the drag started, in the list's own coordinates (scrolling doesn't move it). */
type Slot = { el: HTMLElement; kind: "channel" | "category"; id: string; parent: string; collapsed: boolean; top: number; bottom: number };

type Mark = { key: string; drop: Drop | null; line?: number; ring?: Slot };

type Dragging = {
  id: string;
  pointer: { x: number; y: number };
  drop: Drop | null;
  land: (root: HTMLElement, moved: boolean) => void;
};

const EDGE = 48;

function start(root: HTMLElement, row: HTMLElement, x: number, y: number, layout: Layout): Dragging {
  const calm = reduceMotion();
  const kind = row.dataset.arrange as "channel" | "category";
  const id = row.dataset.id!;
  const box = root.getBoundingClientRect();
  const all: Slot[] = [...root.querySelectorAll<HTMLElement>("[data-arrange]")].map((el) => {
    const r = el.getBoundingClientRect();
    return {
      el,
      kind: el.dataset.arrange as Slot["kind"],
      id: el.dataset.id!,
      parent: el.dataset.parent ?? "",
      collapsed: el.hasAttribute("data-collapsed"),
      top: r.top - box.top + root.scrollTop,
      bottom: r.bottom - box.top + root.scrollTop,
    };
  });
  // What moves: the row, and a category's channels with it.
  const moving = all.filter((s) => s.id === id || (kind === "category" && s.parent === id));
  for (const s of moving) s.el.setAttribute("data-dragging", "");
  const movingSet = new Set(moving);
  const rest = all.filter((s) => !movingSet.has(s));

  // The copy under the pointer.
  const from = row.getBoundingClientRect();
  const ghost = document.createElement("div");
  ghost.className = "arrange-ghost";
  ghost.setAttribute("aria-hidden", "true");
  const copy = row.cloneNode(true) as HTMLElement;
  for (const el of [copy, ...copy.querySelectorAll("[id],[data-dragging],[data-pressing]")]) {
    el.removeAttribute("id");
    el.removeAttribute("data-dragging");
    el.removeAttribute("data-pressing");
    el.removeAttribute("data-arrange");
  }
  copy.style.transform = "none";
  copy.style.opacity = "1";
  ghost.append(copy);
  if (kind === "category") {
    const count = moving.length - 1 || (layout.categories.find((c) => c.id === id)?.children.length ?? 0);
    ghost.classList.add("stacked");
    const badge = document.createElement("span");
    badge.className = "arrange-count";
    badge.textContent = String(count);
    if (count) ghost.append(badge);
  }
  ghost.style.width = `${from.width}px`;
  document.body.append(ghost);
  // Held a little off the pointer so the drop line stays in sight (above a finger, which would cover it).
  const touch = row.hasAttribute("data-touch");
  row.removeAttribute("data-touch");
  const grab = { x: x - from.left - 14, y: y - from.top + (touch ? 34 : -10) };
  let at = { x: from.left, y: from.top };
  let tilt = 0;
  requestAnimationFrame(() => ghost.classList.add("lifted"));

  // The drop line and the category ring, drawn in the list.
  const line = document.createElement("div");
  line.className = "arrange-line";
  const ring = document.createElement("div");
  ring.className = "arrange-ring";
  root.append(line, ring);
  root.setAttribute("data-arranging", kind);
  document.documentElement.setAttribute("data-grabbing", "");

  const drag: Dragging = { id, pointer: { x, y }, drop: null, land };
  let shown = "";

  /** The middle of the space above a row (the rows that stay, by index). */
  const gapAbove = (n: number) => (n > 0 ? (rest[n - 1]!.bottom + rest[n]!.top) / 2 : rest[n]!.top - 3);
  const mark = (py: number): Mark => (kind === "category" ? markCategory(py) : markChannel(py));

  /** Where a category goes: between the other categories' blocks. */
  function markCategory(py: number): Mark {
    const headers = rest.filter((s) => s.kind === "category");
    for (let n = 0; n < headers.length; n++) {
      const h = headers[n]!;
      const end = (headers[n + 1]?.top ?? rest.at(-1)!.bottom) as number;
      if (py < (h.top + end) / 2) return { key: `c:${h.id}`, drop: { kind: "category", id, before: h.id }, line: gapAbove(rest.indexOf(h)) };
    }
    const last = rest.at(-1);
    return { key: "c:end", drop: { kind: "category", id, before: null }, line: (last?.bottom ?? 0) + 2 };
  }

  /** Where a channel goes: before or after a channel, or into a category by its header. */
  function markChannel(py: number): Mark {
    if (!rest.length) return { key: "none", drop: null };
    // The group the space right after row n belongs to.
    const groupAfter = (n: number): string => {
      const s = rest[n];
      if (!s) return "";
      return s.kind === "category" ? s.id : s.parent;
    };
    const firstOf = (category: string) => layout.categories.find((c) => c.id === category)?.children.find((c) => c !== id) ?? null;
    const endOf = (n: number): Mark => {
      const group = groupAfter(n);
      const s = rest[n]!;
      if (s.kind === "category") return s.collapsed ? into(s) : { key: `start:${s.id}`, drop: { kind: "channel", id, parent: s.id, before: firstOf(s.id) }, line: s.bottom + 1 };
      return { key: `after:${s.id}`, drop: { kind: "channel", id, parent: group, before: null }, line: s.bottom + 1 };
    };
    const into = (s: Slot): Mark => ({ key: `into:${s.id}`, drop: { kind: "channel", id, parent: s.id, before: firstOf(s.id) }, ring: s });

    if (py < rest[0]!.top) {
      const s = rest[0]!;
      return s.kind === "channel"
        ? { key: `before:${s.id}`, drop: { kind: "channel", id, parent: s.parent, before: s.id }, line: s.top - 1 }
        : { key: "loose:end", drop: { kind: "channel", id, parent: "", before: null }, line: gapAbove(0) };
    }
    for (let n = 0; n < rest.length; n++) {
      const s = rest[n]!;
      const next = rest[n + 1];
      // A row's own height plus half the gap below it.
      const bottom = next ? (s.bottom + next.top) / 2 : Infinity;
      if (py >= bottom) continue;
      const h = s.bottom - s.top;
      if (s.kind === "channel") {
        if (py < s.top + h / 2) return { key: `before:${s.id}`, drop: { kind: "channel", id, parent: s.parent, before: s.id }, line: s.top - 1 };
        return next?.kind === "channel" && next.parent === s.parent
          ? { key: `before:${next.id}`, drop: { kind: "channel", id, parent: s.parent, before: next.id }, line: s.bottom + 1 }
          : endOf(n);
      }
      // A category's header: its top edge is the end of what's above it.
      if (py < s.top + h * 0.25) {
        if (n === 0) return { key: "loose:end", drop: { kind: "channel", id, parent: "", before: null }, line: gapAbove(0) };
        const above = rest[n - 1]!;
        if (above.kind === "category" && above.collapsed) return into(above);
        return { key: `end:${groupAfter(n - 1)}`, drop: { kind: "channel", id, parent: groupAfter(n - 1), before: null }, line: gapAbove(n) };
      }
      if (py < s.top + h * 0.75 || s.collapsed) return into(s);
      return endOf(n);
    }
    return endOf(rest.length - 1);
  }

  const show = (m: Mark) => {
    if (m.key === shown) return;
    shown = m.key;
    drag.drop = m.drop;
    for (const s of rest) if (s.kind === "category") s.el.toggleAttribute("data-drop-into", m.ring === s);
    if (m.line !== undefined) {
      line.style.transform = `translate3d(0, ${m.line - 1}px, 0)`;
      line.classList.add("on");
    } else {
      line.classList.remove("on");
    }
    if (m.ring) {
      ring.style.height = `${m.ring.bottom - m.ring.top + 4}px`;
      ring.style.transform = `translate3d(0, ${m.ring.top - 2}px, 0)`;
      ring.classList.remove("on");
      void ring.offsetWidth;
      ring.classList.add("on");
    } else {
      ring.classList.remove("on");
    }
  };

  const scroller = scrollerOf(root);
  let frame = requestAnimationFrame(function tick() {
    const p = drag.pointer;
    // Near the top or bottom edge, the list scrolls under the pointer.
    const view = scroller.getBoundingClientRect();
    const top = Math.max(view.top, 0);
    const bottom = Math.min(view.bottom, window.innerHeight);
    const speed = p.y < top + EDGE ? -(top + EDGE - p.y) : p.y > bottom - EDGE ? p.y - (bottom - EDGE) : 0;
    if (speed) scroller.scrollTop += Math.max(-EDGE, Math.min(EDGE, speed)) * 0.35;
    const now = root.getBoundingClientRect();
    show(mark(p.y - now.top + root.scrollTop));
    // The copy trails the pointer a touch and leans the way it's moving.
    const target = { x: Math.max(4, Math.min(p.x - grab.x, window.innerWidth - from.width - 4)), y: p.y - grab.y };
    const follow = calm ? 1 : 0.4;
    const dx = (target.x - at.x) * follow;
    at = { x: at.x + dx, y: at.y + (target.y - at.y) * follow };
    tilt = calm ? 0 : tilt * 0.8 + Math.max(-6, Math.min(6, dx * 0.5)) * 0.2;
    ghost.style.transform = `translate3d(${at.x}px, ${at.y}px, 0) rotate(${tilt}deg)`;
    frame = requestAnimationFrame(tick);
  });

  /** Lets go: the copy flies to where the row is now (or back), then the row takes over. */
  function land(list: HTMLElement, moved: boolean) {
    cancelAnimationFrame(frame);
    line.remove();
    ring.remove();
    list.removeAttribute("data-arranging");
    document.documentElement.removeAttribute("data-grabbing");
    for (const s of rest) s.el.removeAttribute("data-drop-into");
    const done = () => {
      ghost.remove();
      for (const s of moving) {
        s.el.removeAttribute("data-dragging");
        if (moved && s.el.isConnected && s.id === id && !calm) {
          s.el.removeAttribute("data-landed");
          void s.el.offsetWidth;
          s.el.setAttribute("data-landed", "");
          setTimeout(() => s.el.removeAttribute("data-landed"), 700);
        }
      }
    };
    if (calm) return done();
    // Wait for the list to take its new order (and finish most of its reflow) before measuring.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const el = list.querySelector<HTMLElement>(`[data-arrange][data-id="${CSS.escape(id)}"]`);
        const into = el ? null : list.querySelector<HTMLElement>(`[data-arrange="category"][data-id="${CSS.escape(drag.drop && "parent" in drag.drop ? drag.drop.parent : "")}"]`);
        const to = el ?? into ? settled((el ?? into)!, list) : null;
        ghost.classList.remove("lifted");
        ghost.classList.add("landing");
        if (to) {
          const scale = el ? 1 : 0.4;
          ghost.style.transform = `translate3d(${to.left}px, ${to.top + (el ? 0 : to.height / 3)}px, 0) scale(${scale})`;
        }
        if (!el) ghost.style.opacity = "0";
        let finished = false;
        const end = () => {
          if (finished) return;
          finished = true;
          done();
        };
        ghost.addEventListener("transitionend", end, { once: true });
        setTimeout(end, 320);
      }),
    );
  }

  return drag;
}

/**
 * Where a row is once the list stops moving: rows glide to new places with a
 * transform, so this goes by layout offsets, which transforms don't touch.
 */
function settled(el: HTMLElement, root: HTMLElement) {
  let top = 0;
  let left = 0;
  for (let at: HTMLElement | null = el; at && at !== root; at = at.offsetParent as HTMLElement | null) {
    top += at.offsetTop;
    left += at.offsetLeft;
    if (!root.contains(at.offsetParent)) break;
  }
  const box = root.getBoundingClientRect();
  return { top: box.top + root.clientTop + top - root.scrollTop, left: box.left + root.clientLeft + left - root.scrollLeft, height: el.offsetHeight };
}

/** The list itself when it scrolls, or whatever around it does. */
function scrollerOf(root: HTMLElement): HTMLElement {
  for (let el: HTMLElement | null = root; el; el = el.parentElement) {
    const { overflowY } = getComputedStyle(el);
    if ((overflowY === "auto" || overflowY === "scroll") && el.scrollHeight > el.clientHeight) return el;
  }
  return (document.scrollingElement as HTMLElement | null) ?? root;
}
