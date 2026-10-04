import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import { keyOf, moveRail, sameRail, type RailDrop, type RailLayout } from "@/lib/rail";
import { reduceMotion } from "@/lib/prefs";

/**
 * Dragging servers and folders into place on one instance's part of the rail.
 * Inside `container` (positioned), in display order:
 *
 * - every server: `data-rail="server"`, `data-id`, `data-folder` (its folder, or "")
 * - every folder's icon: `data-rail="folder"`, `data-id`, and `data-open` while open
 * - each top-level place, server or folder: `data-rail-unit`, `data-id`; a
 *   folder's holds its icon, its servers while open, and `[data-rail-bg]`
 *
 * Press and move (or hold a moment, on a touch screen) to pick one up. A
 * server goes between any two, into a folder (onto a closed one's icon, or
 * between an open one's servers), or onto another server to make a folder of
 * the two. A folder goes between the top-level places.
 *
 * Everything during a drag happens outside React: the lifted copy follows the
 * pointer, and the others slide aside on `transform` only when the place it
 * would land changes. React hears about it once, when you let go.
 */
export function useRailArrange({
  container,
  enabled,
  layout,
  onArrange,
}: {
  container: RefObject<HTMLElement | null>;
  enabled: boolean;
  layout: RailLayout;
  onArrange: (next: RailLayout, drop: RailDrop) => void;
}) {
  const latest = useRef({ layout, onArrange });
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
      const row = (e.target as Element).closest<HTMLElement>("[data-rail]");
      if (!row || !root.contains(row) || row.closest("[data-motion-pop-id]")) return;
      const touch = e.pointerType !== "mouse";
      pending = {
        row,
        x: e.clientX,
        y: e.clientY,
        pointer: e.pointerId,
        // Touch waits for a hold, so a swipe still scrolls the rail.
        timer: touch ? window.setTimeout(() => begin(), 320) : 0,
        touch,
      };
      if (touch) row.setAttribute("data-pressing", "");
    };

    const begin = () => {
      if (!pending) return;
      const { row, x, y, touch } = pending;
      cancelPending();
      drag = start(root, row, x, y, touch, latest.current.layout);
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
      const now = latest.current.layout;
      const next = commit && d.drop ? moveRail(now, d.drop) : null;
      const changed = !!next && !sameRail(next, now);
      if (changed) {
        // The next render puts everything in its new place; nothing should glide there from the old one.
        root.setAttribute("data-rail-settling", "");
        landing = d.drop!.id;
        latest.current.onArrange(next, d.drop!);
      }
      d.land(changed);
      // The click that ends a mouse drag mustn't open the server.
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
        e.preventDefault();
        e.stopPropagation();
        finish(false);
      }
    };
    // Links drag their address by default; servers here move instead.
    const onNativeDrag = (e: DragEvent) => {
      if ((e.target as Element).closest?.("[data-rail]")) e.preventDefault();
    };
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
  }, [container, enabled]);
}

/**
 * Glides the rail's servers and folders from where they were to where they
 * are after each render (joining, leaving, a folder opening, a move made with
 * the keyboard or on another device), on `transform`; a folder's backdrop
 * grows or shrinks with it. Not after a drag: those land where they already
 * show.
 */
export function useRailGlide(container: RefObject<HTMLElement | null>) {
  const last = useRef(new Map<string, number>());
  const heights = useRef(new Map<string, number>());
  useLayoutEffect(() => {
    const root = container.current;
    if (!root) return;
    const settling = root.hasAttribute("data-rail-settling");
    root.removeAttribute("data-rail-settling");
    const calm = reduceMotion();
    const seen = new Map<string, number>();
    for (const el of parts(root)) {
      const unit = el.hasAttribute("data-rail-unit");
      const within = unit ? root : (el.closest<HTMLElement>("[data-rail-unit]") ?? root);
      const key = `${unit ? "u" : el.dataset.rail}:${el.dataset.id}:${el.dataset.folder ?? ""}`;
      const top = offsetWithin(el, within);
      seen.set(key, top);
      if (settling) {
        clearMoves(el);
        continue;
      }
      const was = last.current.get(key);
      if (was === undefined || was === top || calm) continue;
      el.animate([{ transform: `translate3d(0, ${was - top}px, 0)` }, { transform: "none" }], {
        duration: GLIDE.duration,
        easing: GLIDE.easing,
        composite: "add",
      });
    }
    last.current = seen;
    const sized = new Map<string, number>();
    for (const bg of root.querySelectorAll<HTMLElement>("[data-rail-bg]")) {
      const id = bg.closest<HTMLElement>("[data-rail-unit]")?.dataset.id ?? "";
      if (settling) clearMoves(bg);
      const h = bg.offsetHeight;
      sized.set(id, h);
      const was = heights.current.get(id);
      if (settling || calm || was === undefined || was === h) continue;
      bg.animate([{ height: `${was}px` }, { height: `${h}px` }], { duration: GLIDE.duration, easing: GLIDE.easing });
    }
    heights.current = sized;
  });
}

/** The server or folder just let go of, while its copy flies to it: shown only once that lands. */
let landing: string | null = null;
export const landingId = () => landing;

type Pending = { row: HTMLElement; x: number; y: number; pointer: number; timer: number; touch: boolean };

type Dragging = {
  pointer: { x: number; y: number };
  drop: RailDrop | null;
  land: (moved: boolean) => void;
};

/** Something on the rail as it was when the drag started, in the container's own coordinates. */
type Slot = {
  el: HTMLElement;
  kind: "server" | "folder" | "unit";
  id: string;
  folder: string;
  open: boolean;
  top: number;
  bottom: number;
};

/** Where the lifted one would go: `at` is its place among the others (they slide to open it); `ring` marks one to drop onto. */
type Mark = { key: string; drop: RailDrop | null; at: number; ring?: Slot };

const EDGE = 40;

function start(root: HTMLElement, row: HTMLElement, x: number, y: number, touch: boolean, layout: RailLayout): Dragging {
  const calm = reduceMotion();
  const kind = row.dataset.rail as "server" | "folder";
  const id = row.dataset.id!;
  const box = root.getBoundingClientRect();
  const slot = (el: HTMLElement, as?: Slot["kind"]): Slot => {
    const r = el.getBoundingClientRect();
    return {
      el,
      kind: as ?? (el.dataset.rail as Slot["kind"]),
      id: el.dataset.id!,
      folder: el.dataset.folder ?? "",
      open: el.hasAttribute("data-open"),
      top: r.top - box.top + root.scrollTop,
      bottom: r.bottom - box.top + root.scrollTop,
    };
  };
  const live = (el: HTMLElement) => !el.closest("[data-motion-pop-id]");
  // A folder moves whole, among the top-level places; a server among every server and folder icon.
  const lifted = kind === "folder" ? root.querySelector<HTMLElement>(`[data-rail-unit][data-id="${CSS.escape(id)}"]`)! : row;
  const all =
    kind === "folder"
      ? [...root.querySelectorAll<HTMLElement>("[data-rail-unit]")].filter(live).map((el) => slot(el, "unit"))
      : [...root.querySelectorAll<HTMLElement>("[data-rail]")].filter(live).map((el) => slot(el));
  const me = all.find((s) => s.el === lifted)!;
  const rest = all.filter((s) => s !== me);
  const from = rest.filter((s) => s.top < me.top).length;
  // How far the others slide: the lifted one's height and the gap after it.
  const gap = (() => {
    const after = all[all.indexOf(me) + 1];
    return after && after.top > me.bottom ? Math.min(after.top - me.bottom, 16) : 8;
  })();
  const size = me.bottom - me.top + gap;
  const sourceFolder = kind === "server" ? me.folder : "";
  // Open folders' backdrops follow their icon and stretch to their servers.
  const backdrops = [...root.querySelectorAll<HTMLElement>("[data-rail-bg]")].filter(live).map((bg) => {
    const unit = bg.closest<HTMLElement>("[data-rail-unit]")!;
    return { bg, id: unit.dataset.id!, height: bg.offsetHeight };
  });

  lifted.setAttribute("data-rail-lifted", "");
  root.setAttribute("data-rail-dragging", kind);
  root.setAttribute("data-rail-sliding", "");
  document.documentElement.setAttribute("data-grabbing", "");

  // The copy under the pointer.
  const r = lifted.getBoundingClientRect();
  const ghost = document.createElement("div");
  ghost.className = "rail-ghost";
  ghost.setAttribute("aria-hidden", "true");
  const copy = lifted.cloneNode(true) as HTMLElement;
  for (const el of [copy, ...copy.querySelectorAll<HTMLElement>("*")]) {
    el.removeAttribute("id");
    for (const a of ["data-rail", "data-rail-unit", "data-rail-lifted", "data-pressing", "data-rail-bg"]) el.removeAttribute(a);
  }
  copy.style.transform = "none";
  ghost.append(copy);
  ghost.style.width = `${r.width}px`;
  document.body.append(ghost);
  // Held where it was picked up; on a touch screen a little above the finger, which would cover it.
  const grab = { x: x - r.left, y: y - r.top + (touch ? 28 : 0) };
  let at = { x: r.left, y: r.top };
  let tilt = 0;
  requestAnimationFrame(() => ghost.classList.add("lifted"));

  const drag: Dragging = { pointer: { x, y }, drop: null, land };
  let shown: Mark = { key: "", drop: null, at: from };

  /** How far each of the others sits from its place while the lifted one would land at `place`. */
  const shift = (n: number, place: number) => (n >= from ? -size : 0) + (n >= place ? size : 0);
  const nextTopLevel = (after: string): string | null => {
    const keys = layout.map(keyOf).filter((k) => k !== id);
    const n = keys.indexOf(after);
    return n === -1 ? null : (keys[n + 1] ?? null);
  };
  const folderServers = (folder: string) => {
    const e = layout.find((x) => x.kind === "folder" && x.folder.id === folder);
    return e?.kind === "folder" ? e.folder.servers.filter((s) => s !== id) : [];
  };

  /** Where the pointer (in container coordinates) would put the lifted one, going by where the others show now. */
  function mark(py: number): Mark {
    if (!rest.length) return { key: "none", drop: null, at: 0 };
    const place = shown.at;
    const tops = rest.map((s, n) => s.top + shift(n, place));
    const hit = rest.findIndex((s, n) => py >= tops[n]! && py < tops[n]! + (s.bottom - s.top));
    if (hit === -1) {
      if (py < tops[0]!) return before(0);
      const last = rest.length - 1;
      const end = tops[last]! + (rest[last]!.bottom - rest[last]!.top);
      if (py > end + size / 2) return kind === "folder" ? folderAt(rest.length) : { key: "end", drop: { kind: "server", id, folder: "", before: null }, at: rest.length };
      // In a gap: the place that's open stays.
      return shown;
    }
    const s = rest[hit]!;
    const f = (py - tops[hit]!) / (s.bottom - s.top);
    if (kind === "folder") return f < 0.5 ? folderAt(hit) : folderAt(hit + 1);
    if (s.kind === "server" && !s.folder) {
      if (f < 0.25) return before(hit);
      if (f > 0.75) return { key: `after:${s.id}`, drop: { kind: "server", id, folder: "", before: nextTopLevel(s.id) }, at: hit + 1 };
      return { key: `with:${s.id}`, drop: { kind: "combine", id, with: s.id }, at: place, ring: s };
    }
    if (s.kind === "server") {
      if (f < 0.5) return before(hit);
      const siblings = folderServers(s.folder);
      const after = siblings[siblings.indexOf(s.id) + 1] ?? null;
      return { key: `in:${s.folder}:${after}`, drop: { kind: "server", id, folder: s.folder, before: after }, at: hit + 1 };
    }
    // A folder's icon: its top edge is the place above it; the rest goes in.
    if (f < (s.open ? 0.3 : 0.25)) return before(hit);
    if (!s.open) return { key: `into:${s.id}`, drop: { kind: "server", id, folder: s.id, before: null }, at: place, ring: s };
    const first = folderServers(s.id)[0] ?? null;
    return { key: `in:${s.id}:${first}`, drop: { kind: "server", id, folder: s.id, before: first }, at: hit + 1 };
  }

  /** Just above the n-th of the others. */
  function before(n: number): Mark {
    const s = rest[n]!;
    if (s.kind === "server" && s.folder) return { key: `in:${s.folder}:${s.id}`, drop: { kind: "server", id, folder: s.folder, before: s.id }, at: n };
    return { key: `before:${s.id}`, drop: { kind: "server", id, folder: "", before: s.id }, at: n };
  }

  function folderAt(n: number): Mark {
    const s = rest[n];
    return { key: `folder:${s?.id ?? "end"}`, drop: { kind: "folder", id, before: s?.id ?? null }, at: n };
  }

  const show = (m: Mark) => {
    if (m.key === shown.key) return;
    const moved = m.at !== shown.at;
    shown = m;
    drag.drop = m.drop;
    for (const s of rest) {
      s.el.toggleAttribute("data-rail-target", m.ring === s);
    }
    ghost.classList.toggle("over", !!m.ring);
    if (!moved) return;
    rest.forEach((s, n) => {
      const by = shift(n, m.at);
      s.el.style.transform = by ? `translate3d(0, ${by}px, 0)` : "";
    });
    for (const b of backdrops) {
      const icon = rest.findIndex((s) => s.kind === "folder" && s.id === b.id);
      if (icon === -1) continue;
      const into = m.drop?.kind === "server" && m.drop.folder === b.id && !m.ring;
      const grow = (into ? size : 0) - (sourceFolder === b.id ? size : 0);
      const by = shift(icon, m.at);
      b.bg.style.transform = by ? `translate3d(0, ${by}px, 0)` : "";
      b.bg.style.height = grow ? `${b.height + grow}px` : "";
    }
  };

  const scroller = scrollerOf(root);
  let frame = requestAnimationFrame(function tick() {
    const p = drag.pointer;
    // Near the top or bottom edge, the rail scrolls under the pointer.
    const view = scroller.getBoundingClientRect();
    const top = Math.max(view.top, 0);
    const bottom = Math.min(view.bottom, window.innerHeight);
    const speed = p.y < top + EDGE ? -(top + EDGE - p.y) : p.y > bottom - EDGE ? p.y - (bottom - EDGE) : 0;
    if (speed) scroller.scrollTop += Math.max(-EDGE, Math.min(EDGE, speed)) * 0.35;
    const now = root.getBoundingClientRect();
    show(mark(p.y - now.top + root.scrollTop));
    // The copy trails the pointer a touch and leans the way it's moving.
    const target = { x: p.x - grab.x, y: p.y - grab.y };
    const follow = calm ? 1 : 0.38;
    const dy = (target.y - at.y) * follow;
    at = { x: at.x + (target.x - at.x) * follow, y: at.y + dy };
    tilt = calm ? 0 : tilt * 0.82 + Math.max(-8, Math.min(8, dy * 0.6)) * 0.18;
    ghost.style.transform = `translate3d(${at.x}px, ${at.y}px, 0) rotate(${tilt}deg)`;
    frame = requestAnimationFrame(tick);
  });

  /** Lets go: the copy flies to where it now is (or back, or into its folder), then the real one shows. */
  function land(moved: boolean) {
    cancelAnimationFrame(frame);
    root.removeAttribute("data-rail-dragging");
    document.documentElement.removeAttribute("data-grabbing");
    for (const s of rest) s.el.removeAttribute("data-rail-target");
    if (!moved) {
      // Everything slides back where it was.
      for (const s of rest) s.el.style.transform = "";
      for (const b of backdrops) {
        b.bg.style.transform = "";
        b.bg.style.height = "";
      }
    }
    let hidden: HTMLElement | null = moved ? null : lifted;
    const done = () => {
      ghost.remove();
      root.removeAttribute("data-rail-sliding");
      landing = null;
      lifted.removeAttribute("data-rail-lifted");
      hidden?.removeAttribute("data-rail-lifted");
      for (const s of all) clearMoves(s.el);
      for (const b of backdrops) clearMoves(b.bg);
    };
    if (calm) return done();
    // Wait for the rail to take its new order before measuring.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const selector = kind === "folder" ? `[data-rail-unit][data-id="${CSS.escape(id)}"]` : `[data-rail="server"][data-id="${CSS.escape(id)}"]`;
        const el = [...root.querySelectorAll<HTMLElement>(selector)].find((x) => !x.closest("[data-motion-pop-id]")) ?? null;
        // Into a closed folder: it flies to the folder's icon and shrinks away there.
        const folder = !el && drag.drop?.kind === "server" ? drag.drop.folder : "";
        const into = folder ? root.querySelector<HTMLElement>(`[data-rail="folder"][data-id="${CSS.escape(folder)}"]`) : null;
        if (el && el !== lifted) {
          hidden = el;
          el.setAttribute("data-rail-lifted", "");
        }
        const to = (el ?? into)?.getBoundingClientRect();
        ghost.classList.remove("lifted", "over");
        ghost.classList.add("landing");
        if (to) {
          const scale = el ? 1 : 0.35;
          ghost.style.transform = `translate3d(${to.left + (el ? 0 : to.width * 0.32)}px, ${to.top + (el ? 0 : to.height * 0.32)}px, 0) scale(${scale})`;
        }
        if (!el) ghost.style.opacity = "0";
        if (into) {
          into.removeAttribute("data-rail-took");
          void into.offsetWidth;
          into.setAttribute("data-rail-took", "");
          setTimeout(() => into.removeAttribute("data-rail-took"), 600);
        }
        let finished = false;
        const end = () => {
          if (finished) return;
          finished = true;
          done();
        };
        ghost.addEventListener("transitionend", end, { once: true });
        setTimeout(end, 340);
      }),
    );
  }

  return drag;
}

/** The rail's servers, folder icons and top-level places, leaving out ones on their way out. */
function parts(root: HTMLElement) {
  return [...root.querySelectorAll<HTMLElement>("[data-rail], [data-rail-unit]")].filter((el) => !el.closest("[data-motion-pop-id]"));
}

function clearMoves(el: HTMLElement) {
  el.style.transition = "none";
  el.style.transform = "";
  el.style.height = "";
  void el.offsetWidth;
  el.style.transition = "";
}

/** How far down `within` (a positioned ancestor) an element sits, by layout offsets, which transforms don't move. */
function offsetWithin(el: HTMLElement, within: HTMLElement) {
  let top = 0;
  for (let at: HTMLElement | null = el; at && at !== within; at = at.offsetParent as HTMLElement | null) {
    if (!within.contains(at)) return top;
    top += at.offsetTop;
  }
  return top;
}

/** The rail itself when it scrolls, or whatever around it does. */
function scrollerOf(root: HTMLElement): HTMLElement {
  for (let el: HTMLElement | null = root; el; el = el.parentElement) {
    const { overflowY } = getComputedStyle(el);
    if ((overflowY === "auto" || overflowY === "scroll") && el.scrollHeight > el.clientHeight) return el;
  }
  return (document.scrollingElement as HTMLElement | null) ?? root;
}

/** A spring as a CSS easing, for glides the browser runs by itself. */
function spring(stiffness: number, damping: number): { easing: string; duration: number } {
  const points: number[] = [];
  let x = 0;
  let v = 0;
  const dt = 1 / 120;
  let t = 0;
  for (; t < 2; t += dt) {
    const a = -stiffness * (x - 1) - damping * v;
    v += a * dt;
    x += v * dt;
    points.push(x);
    if (t > 0.1 && Math.abs(x - 1) < 0.001 && Math.abs(v) < 0.01) break;
  }
  const step = Math.max(1, Math.floor(points.length / 48));
  const kept = points.filter((_, n) => n % step === 0);
  kept[kept.length - 1] = 1;
  return { easing: `linear(0, ${kept.map((p) => +p.toFixed(4)).join(", ")})`, duration: Math.round(t * 1000) };
}

/** The app's spring (components/motion `SPRING`), for glides. */
const GLIDE = spring(520, 34);
