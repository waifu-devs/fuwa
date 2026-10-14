import { useEffect, useRef, type RefObject } from "react";

/**
 * Dragging a board's cards between and within its columns. Like
 * `use-arrange.ts` it works on the DOM, not React state: cards are marked
 * `data-card` with `data-id`, each column `data-column` with `data-id` and
 * its list `data-cards`. While dragging, a copy of the card follows the
 * pointer and a line shows where it will land, both moved by `transform`,
 * so React renders once, when `onMove` runs on the drop. A mouse drags
 * after moving a few pixels; a finger after holding still a moment. The
 * board and the column under the pointer scroll near their edges. Escape
 * puts it back.
 */
export function useCardDrag({
  container,
  enabled,
  onMove,
}: {
  container: RefObject<HTMLElement | null>;
  enabled: boolean;
  /** A card dropped above `beforeId` in a column ("" for its bottom). */
  onMove: (cardId: string, columnId: string, beforeId: string) => void;
}) {
  const move = useRef(onMove);
  useEffect(() => {
    move.current = onMove;
  }, [onMove]);

  useEffect(() => {
    const root = container.current;
    if (!root || !enabled) return;

    type Drag = {
      card: HTMLElement;
      id: string;
      pointer: number;
      startX: number;
      startY: number;
      dx: number;
      dy: number;
      ghost: HTMLElement | null;
      line: HTMLElement | null;
      target: { columnId: string; beforeId: string } | null;
      hold: ReturnType<typeof setTimeout> | null;
      armed: boolean;
      x: number;
      y: number;
      frame: number;
    };
    let drag: Drag | null = null;
    let swallowClick = false;

    const begin = (d: Drag) => {
      const rect = d.card.getBoundingClientRect();
      d.dx = d.startX - rect.left;
      d.dy = d.startY - rect.top;
      const ghost = d.card.cloneNode(true) as HTMLElement;
      ghost.removeAttribute("data-card");
      Object.assign(ghost.style, {
        position: "fixed",
        left: "0px",
        top: "0px",
        width: `${rect.width}px`,
        height: `${rect.height}px`,
        margin: "0",
        pointerEvents: "none",
        zIndex: "60",
        transition: "box-shadow 150ms, rotate 150ms, scale 150ms",
        rotate: "2deg",
        scale: "1.03",
        boxShadow: "0 18px 40px rgb(0 0 0 / 0.35)",
        willChange: "transform",
      });
      document.body.appendChild(ghost);
      const line = document.createElement("div");
      line.className = "board-drop-line";
      Object.assign(line.style, { position: "fixed", left: "0px", top: "0px", height: "3px", pointerEvents: "none", zIndex: "59", display: "none" });
      document.body.appendChild(line);
      d.card.dataset.dragging = "true";
      d.ghost = ghost;
      d.line = line;
      d.armed = true;
      document.body.style.userSelect = "none";
      place(d);
      d.frame = requestAnimationFrame(() => tick(d));
    };

    /** Where the pointer is: the column under it, and the card it's above. */
    const place = (d: Drag) => {
      if (!d.ghost || !d.line) return;
      d.ghost.style.transform = `translate(${d.x - d.dx}px, ${d.y - d.dy}px)`;
      const under = document.elementsFromPoint(d.x, d.y).find((el) => el instanceof HTMLElement && el.closest("[data-column]") && root.contains(el));
      const column = (under as HTMLElement | undefined)?.closest<HTMLElement>("[data-column]");
      const list = column?.querySelector<HTMLElement>("[data-cards]");
      if (!column || !list) {
        d.target = null;
        d.line.style.display = "none";
        return;
      }
      const cards = [...list.querySelectorAll<HTMLElement>("[data-card]")].filter((el) => el.dataset.id !== d.id);
      const before = cards.find((el) => {
        const r = el.getBoundingClientRect();
        return d.y < r.top + r.height / 2;
      });
      const box = list.getBoundingClientRect();
      let y: number;
      if (before) y = before.getBoundingClientRect().top - 5;
      else if (cards.length) y = cards[cards.length - 1]!.getBoundingClientRect().bottom + 3;
      else y = box.top + 4;
      Object.assign(d.line.style, { display: "block", width: `${box.width}px`, transform: `translate(${box.left}px, ${y}px)` });
      d.target = { columnId: column.dataset.id ?? "", beforeId: before?.dataset.id ?? "" };
    };

    /** Scrolls the board and the column under the pointer near their edges, each frame. */
    const tick = (d: Drag) => {
      if (drag !== d) return;
      const edge = 48;
      const speed = (gap: number) => Math.ceil(((edge - gap) / edge) * 14);
      const r = root.getBoundingClientRect();
      if (d.x < r.left + edge) root.scrollLeft -= speed(d.x - r.left);
      else if (d.x > r.right - edge) root.scrollLeft += speed(r.right - d.x);
      const list = document.elementsFromPoint(d.x, d.y).map((el) => (el as HTMLElement).closest?.("[data-cards]")).find(Boolean) as HTMLElement | undefined;
      if (list) {
        const b = list.getBoundingClientRect();
        if (d.y < b.top + edge) list.scrollTop -= speed(d.y - b.top);
        else if (d.y > b.bottom - edge) list.scrollTop += speed(b.bottom - d.y);
      }
      place(d);
      d.frame = requestAnimationFrame(() => tick(d));
    };

    const end = (drop: boolean) => {
      const d = drag;
      drag = null;
      if (!d) return;
      if (d.hold) clearTimeout(d.hold);
      cancelAnimationFrame(d.frame);
      d.ghost?.remove();
      d.line?.remove();
      delete d.card.dataset.dragging;
      document.body.style.userSelect = "";
      if (!d.armed) return;
      swallowClick = true;
      setTimeout(() => (swallowClick = false), 0);
      if (!drop || !d.target) return;
      // Dropped where it was: nothing to do.
      const next = d.card.nextElementSibling as HTMLElement | null;
      const home = d.card.closest<HTMLElement>("[data-column]")?.dataset.id;
      if (home === d.target.columnId && (next?.dataset.id ?? "") === d.target.beforeId) return;
      move.current(d.id, d.target.columnId, d.target.beforeId);
    };

    const onDown = (e: PointerEvent) => {
      if (e.button !== 0 || drag) return;
      const target = e.target as HTMLElement;
      if (target.closest("button, a, input, textarea, [data-no-drag]")) return;
      const card = target.closest<HTMLElement>("[data-card]");
      if (!card || !root.contains(card)) return;
      const d: Drag = {
        card,
        id: card.dataset.id ?? "",
        pointer: e.pointerId,
        startX: e.clientX,
        startY: e.clientY,
        dx: 0,
        dy: 0,
        ghost: null,
        line: null,
        target: null,
        hold: null,
        armed: false,
        x: e.clientX,
        y: e.clientY,
        frame: 0,
      };
      drag = d;
      if (e.pointerType !== "mouse") d.hold = setTimeout(() => drag === d && begin(d), 320);
    };

    const onMovePointer = (e: PointerEvent) => {
      const d = drag;
      if (!d || e.pointerId !== d.pointer) return;
      d.x = e.clientX;
      d.y = e.clientY;
      if (d.armed) {
        e.preventDefault();
        return;
      }
      const far = Math.hypot(e.clientX - d.startX, e.clientY - d.startY) > 6;
      // A finger that moves before the hold is scrolling, not dragging.
      if (far && e.pointerType !== "mouse") return end(false);
      if (far) begin(d);
    };

    const onUp = (e: PointerEvent) => {
      if (drag && e.pointerId === drag.pointer) end(true);
    };
    const onCancel = () => end(false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && drag?.armed) {
        e.preventDefault();
        end(false);
      }
    };
    const onClick = (e: MouseEvent) => {
      if (swallowClick) {
        e.stopPropagation();
        e.preventDefault();
      }
    };
    // Touch moves while holding a card shouldn't scroll the page.
    const onTouchMove = (e: TouchEvent) => {
      if (drag?.armed) e.preventDefault();
    };

    root.addEventListener("pointerdown", onDown);
    window.addEventListener("pointermove", onMovePointer, { passive: false });
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onKey, true);
    root.addEventListener("click", onClick, true);
    root.addEventListener("touchmove", onTouchMove, { passive: false });
    return () => {
      end(false);
      root.removeEventListener("pointerdown", onDown);
      window.removeEventListener("pointermove", onMovePointer);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("keydown", onKey, true);
      root.removeEventListener("click", onClick, true);
      root.removeEventListener("touchmove", onTouchMove);
    };
  }, [container, enabled]);
}
