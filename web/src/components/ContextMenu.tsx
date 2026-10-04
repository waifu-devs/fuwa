import { useEffect, useMemo, useRef, useSyncExternalStore, type KeyboardEvent, type MouseEvent, type PointerEvent } from "react";
import { createPortal } from "react-dom";
import { useNavigate } from "@tanstack/react-router";
import { useFuwa } from "@/fuwa/store";
import { MenuDialogs } from "@/components/menus/MenuDialogs";
import { setMenuNavigate } from "@/components/menus/common";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { drifted, HOLD_MS, keyboardPoint, opensMenu, type MenuEntry, type MenuPoint, type MenuSection } from "@/lib/context-menu";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { reportTiming, reportUsage } from "@/lib/reports";
import { cn } from "@/lib/utils";

/*
 * The right-click menu: one at a time, drawn by `ContextMenuHost` with the
 * app's dropdown menu (Radix, animated), anchored at the pointer. Anything on
 * screen gets one with `useContextMenu`, which opens it on a right click, a
 * held finger, or Shift+F10 / the Menu key while something in it has focus.
 * Shift + right click still gives the browser's own menu.
 */

/** What opened a menu: where, on what, and how. */
export type MenuTrigger = {
  /** The element the menu is for (the one with the handlers). */
  element: HTMLElement;
  /** What exactly was clicked inside it, such as a link or a picture. */
  target: Element;
  point: MenuPoint;
  via: "mouse" | "touch" | "keyboard";
  /** Text selected inside the element when it opened. */
  selection: string;
};

type OpenMenu = {
  id: number;
  /** For anonymous counts: which menu, like "message". */
  kind: string;
  point: MenuPoint;
  via: MenuTrigger["via"];
  element: HTMLElement;
  /** Where focus goes back to when it closes from the keyboard. */
  returnFocus: HTMLElement | null;
  build: () => MenuSection[];
  /** When the click came in, to time how long the menu took to show. */
  startedAt: number;
};

/** The open menu, and any still playing their exit (a new one opens as its own menu, at its own place). */
type Menus = { current: OpenMenu | null; shown: OpenMenu[] };
let menus: Menus = { current: null, shown: [] };
let nextId = 1;
const listeners = new Set<() => void>();
const set = (next: Menus) => {
  menus = next;
  for (const l of listeners) l();
};
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};
/** How long a closed menu stays drawn for its exit. */
const EXIT_MS = 260;

function showMenu(menu: OpenMenu) {
  set({ current: menu, shown: [...menus.shown, menu] });
}

export function closeContextMenu() {
  const closing = menus.current;
  if (!closing) return;
  set({ current: null, shown: menus.shown });
  setTimeout(() => set({ ...menus, shown: menus.shown.filter((m) => m !== closing) }), EXIT_MS);
}

const isEditable = (el: Element | null) => !!el?.closest("input, textarea, select, [contenteditable=''], [contenteditable='true']");

/** The selected text, if the selection is inside `el`. */
function selectionIn(el: HTMLElement): string {
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || !sel.rangeCount) return "";
  const range = sel.getRangeAt(0);
  return el.contains(range.commonAncestorContainer) ? sel.toString() : "";
}

/** A click right after a held finger lets go mustn't also open what it was held on. */
function swallowNextClick() {
  const swallow = (ev: Event) => {
    ev.preventDefault();
    ev.stopPropagation();
  };
  window.addEventListener("click", swallow, { capture: true, once: true });
  setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 400);
}

/** A finger held down on something with a menu. */
type Press = {
  pointer: number;
  start: MenuPoint;
  at: number;
  element: HTMLElement;
  target: Element;
  timer: number;
  /** Rows that drag into order on a hold open their menu on letting go instead, if the finger never moved. */
  onRelease: boolean;
  opened: boolean;
  squeeze: Animation | null;
  open: (t: MenuTrigger) => boolean;
  stop: () => void;
};
let press: Press | null = null;
/** When a menu last opened from a key or a finger, so the browser's own contextmenu event that follows is ignored. */
let openedByOtherAt = 0;

function startPress(e: PointerEvent<HTMLElement>, open: (t: MenuTrigger) => boolean) {
  press?.stop();
  const element = e.currentTarget;
  const target = e.target as Element;
  const start = { x: e.clientX, y: e.clientY };
  // The element sinks a little while held, so you can tell the hold is working.
  const squeeze = reduceMotion() ? null : element.animate?.([{ scale: "1" }, { scale: "0.97" }], { duration: HOLD_MS, easing: "ease-out", fill: "forwards" }) ?? null;
  const p: Press = {
    pointer: e.pointerId,
    start,
    at: performance.now(),
    element,
    target,
    timer: 0,
    onRelease: !!target.closest("[data-arrange]"),
    opened: false,
    squeeze,
    open,
    stop() {
      clearTimeout(p.timer);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      if (p.squeeze) {
        p.squeeze.reverse();
        const done = p.squeeze;
        done.onfinish = () => done.cancel();
      }
      if (press === p) press = null;
    },
  };
  const fire = () => {
    if (p.opened) return;
    p.opened = true;
    openedByOtherAt = performance.now();
    navigator.vibrate?.(10);
    if (p.open({ element, target, point: start, via: "touch", selection: "" })) swallowNextClick();
  };
  function onMove(ev: globalThis.PointerEvent) {
    if (ev.pointerId === p.pointer && drifted(start, { x: ev.clientX, y: ev.clientY })) p.stop();
  }
  function onUp(ev: globalThis.PointerEvent) {
    if (ev.pointerId !== p.pointer) return;
    if (p.onRelease && !p.opened && performance.now() - p.at >= HOLD_MS && !drifted(start, { x: ev.clientX, y: ev.clientY })) fire();
    p.stop();
  }
  function onCancel(ev: globalThis.PointerEvent) {
    if (ev.pointerId === p.pointer) p.stop();
  }
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", onCancel);
  if (!p.onRelease) p.timer = window.setTimeout(fire, HOLD_MS);
  press = p;
}

/**
 * Gives an element a right-click menu. `build` says what's in it, called when
 * it opens (and again while open, as what it shows changes); null or no
 * sections leaves the browser's menu alone. Spread what it returns on the
 * element. The handlers never change, so memoized rows keep their props.
 */
export function useContextMenu(kind: string, build: (trigger: MenuTrigger) => MenuSection[] | null, { touch = true }: { touch?: boolean } = {}) {
  const latest = useRef(build);
  useEffect(() => {
    latest.current = build;
  });
  return useMemo(() => {
    const open = (trigger: MenuTrigger, startedAt = performance.now()) => {
      const make = () => latest.current(trigger) ?? [];
      if (!make().length) return false;
      const active = document.activeElement;
      // A menu already open goes first, so this one opens fresh at its own place.
      closeContextMenu();
      showMenu({
        id: nextId++,
        kind,
        point: trigger.point,
        via: trigger.via,
        element: trigger.element,
        returnFocus: active instanceof HTMLElement && active !== document.body ? active : trigger.element,
        build: make,
        startedAt,
      });
      reportUsage(`context_menu.${kind}`);
      return true;
    };
    return {
      "data-context-menu": "",
      onContextMenu(e: MouseEvent<HTMLElement>) {
        // Shift + right click is the way to the browser's own menu (spelling, saving a link).
        if (e.shiftKey && e.button === 2) return;
        // Where a held finger means selecting text (the message box), phones keep their own menu.
        if (!touch && (e.nativeEvent as globalThis.PointerEvent).pointerType === "touch") return;
        // Typing somewhere inside (editing a message) keeps the browser's menu, with its spelling fixes.
        if (e.target !== e.currentTarget && isEditable(e.target as Element)) return;
        // Just opened from a key or a held finger: this is the browser's own event for the same thing.
        if (performance.now() - openedByOtherAt < 600 || press?.opened) {
          e.preventDefault();
          e.stopPropagation();
          return;
        }
        // A finger held on a row that drags: the menu waits for it to let go.
        if (press && press.element === e.currentTarget) {
          e.preventDefault();
          e.stopPropagation();
          if (!press.onRelease) {
            clearTimeout(press.timer);
            press.opened = true;
            openedByOtherAt = performance.now();
            if (open({ element: press.element, target: press.target, point: press.start, via: "touch", selection: "" })) swallowNextClick();
          }
          return;
        }
        const element = e.currentTarget;
        // A contextmenu event from the keyboard has no pointer: put the menu by the element instead.
        const keyboard = e.button !== 2 && e.clientX === 0 && e.clientY === 0;
        const point = keyboard ? keyboardPoint(element.getBoundingClientRect(), { width: innerWidth, height: innerHeight }) : { x: e.clientX, y: e.clientY };
        const opened = open({ element, target: e.target as Element, point, via: keyboard ? "keyboard" : "mouse", selection: selectionIn(element) }, e.timeStamp);
        if (opened) {
          e.preventDefault();
          e.stopPropagation();
        }
      },
      onKeyDown(e: KeyboardEvent<HTMLElement>) {
        if (!opensMenu(e)) return;
        if (e.target !== e.currentTarget && isEditable(e.target as Element)) return;
        const focused = e.target instanceof HTMLElement ? e.target : e.currentTarget;
        const point = keyboardPoint(focused.getBoundingClientRect(), { width: innerWidth, height: innerHeight });
        if (open({ element: e.currentTarget, target: focused, point, via: "keyboard", selection: selectionIn(e.currentTarget) }, e.timeStamp)) {
          e.preventDefault();
          e.stopPropagation();
          openedByOtherAt = performance.now();
        }
      },
      onPointerDown(e: PointerEvent<HTMLElement>) {
        if (!touch || e.pointerType === "mouse" || !e.isPrimary || e.button !== 0) return;
        // Only the innermost element with a menu starts the hold.
        if ((e.nativeEvent as { menuHeld?: boolean }).menuHeld) return;
        (e.nativeEvent as { menuHeld?: boolean }).menuHeld = true;
        if (e.target !== e.currentTarget && isEditable(e.target as Element)) return;
        startPress(e, (t) => open(t));
      },
    };
  }, [kind, touch]);
}

/** Draws whichever menu is open; lives once, with the app's other overlays. */
export function ContextMenuHost() {
  const { current: menu, shown } = useSyncExternalStore(subscribe, () => menus);
  const navigate = useNavigate();

  useEffect(() => {
    setMenuNavigate((options) => void navigate(options as never));
    return () => setMenuNavigate(null);
  }, [navigate]);

  // What the menu is for is lit while it's open, like a row you'd act on.
  useEffect(() => {
    const el = menu?.element;
    if (!el) return;
    el.setAttribute("data-menu-open", "");
    return () => el.removeAttribute("data-menu-open");
  }, [menu]);

  // Scrolling, resizing or leaving the window puts it away, as desktop menus do.
  useEffect(() => {
    if (!menu) return;
    const onScroll = (e: Event) => {
      const t = e.target;
      if (t instanceof Element && t.closest("[data-slot=dropdown-menu-content], [data-slot=dropdown-menu-sub-content]")) return;
      closeContextMenu();
    };
    window.addEventListener("scroll", onScroll, { capture: true, passive: true });
    window.addEventListener("resize", closeContextMenu);
    window.addEventListener("blur", closeContextMenu);
    return () => {
      window.removeEventListener("scroll", onScroll, { capture: true });
      window.removeEventListener("resize", closeContextMenu);
      window.removeEventListener("blur", closeContextMenu);
    };
  }, [menu]);

  return (
    <>
      {shown.map((m) => (
        <OneMenu key={m.id} menu={m} open={m === menu} />
      ))}
      <MenuDialogs />
    </>
  );
}

/** A spring that settles fast: menus are small and should feel instant. */
const POP = { type: "spring", stiffness: 700, damping: 38, mass: 0.6 } as const;

function OneMenu({ menu, open }: { menu: OpenMenu; open: boolean }) {
  return (
    <DropdownMenu open={open} onOpenChange={(o) => !o && menus.current === menu && closeContextMenu()} modal={false}>
      {createPortal(
        <DropdownMenuTrigger asChild>
          <span aria-hidden tabIndex={-1} className="pointer-events-none fixed size-0" style={{ left: menu.point.x, top: menu.point.y }} />
        </DropdownMenuTrigger>,
        document.body,
      )}
      <DropdownMenuContent
        side="bottom"
        align="start"
        sideOffset={2}
        collisionPadding={8}
        loop
        aria-label="Actions"
        onContextMenu={(e) => e.preventDefault()}
        onCloseAutoFocus={(e) => {
          e.preventDefault();
          if (menu.via === "keyboard") menu.returnFocus?.focus({ preventScroll: true });
        }}
        initial={{ opacity: 0, scale: 0.9, y: -4 }}
        animate={{ opacity: 1, scale: 1, y: 0, transition: POP }}
        exit={{ opacity: 0, scale: 0.96, transition: { duration: 0.12 } }}
        className="context-menu w-60 max-w-[calc(100vw-1rem)] rounded-xl p-1.5 shadow-xl"
      >
        <MenuBody menu={menu} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The sections, built again whenever what they show changes (a role given, a mute set) while it's open. */
function MenuBody({ menu }: { menu: OpenMenu }) {
  // Any change on any instance re-runs `build`: menus are small and only one is open.
  useFuwa((s) => s);
  usePrefs((p) => p);
  const sections = menu.build();
  const body = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const raf = requestAnimationFrame(() => {
      reportTiming("context_menu.open", performance.now() - menu.startedAt);
      // From the keyboard, the first item takes focus, ready for the arrows.
      if (menu.via === "keyboard") body.current?.querySelector<HTMLElement>("[role^=menuitem]:not([data-disabled])")?.focus();
    });
    return () => cancelAnimationFrame(raf);
  }, [menu]);
  let n = 0;
  return (
    <div ref={body} className="contents">
      {sections.map((section, s) => (
        <div key={section.id} role="group" data-section={section.id}>
          {s > 0 && <DropdownMenuSeparator className="mx-1" />}
          {section.items.map((entry) => (
            <Entry key={entry.id} entry={entry} index={n++} />
          ))}
        </div>
      ))}
    </div>
  );
}

/** Items come in one after another, quickly, from just above. */
const stagger = (index: number) => ({
  initial: { opacity: 0, y: -3 },
  animate: { opacity: 1, y: 0, transition: { ...POP, delay: Math.min(index, 10) * 0.012 } },
});

function Entry({ entry, index }: { entry: MenuEntry; index: number }) {
  switch (entry.kind) {
    case "note":
      return <p className="px-2 py-1 text-xs text-muted-foreground">{entry.label}</p>;
    case "custom":
      return <>{entry.render(closeContextMenu)}</>;
    case "sub": {
      const Icon = entry.icon;
      return (
        <DropdownMenuSub>
          <DropdownMenuSubTrigger disabled={entry.disabled} {...stagger(index)}>
            {Icon && <Icon />}
            <span className="truncate">{entry.label}</span>
            {entry.hint && <span className="ml-auto truncate pl-2 text-xs text-muted-foreground">{entry.hint}</span>}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent
            sideOffset={4}
            collisionPadding={8}
            initial={{ opacity: 0, x: -8, scale: 0.97 }}
            animate={{ opacity: 1, x: 0, scale: 1, transition: POP }}
            exit={{ opacity: 0, x: -4, transition: { duration: 0.1 } }}
            className="max-h-[min(24rem,var(--radix-dropdown-menu-content-available-height))] w-56 overflow-y-auto rounded-xl p-1.5 shadow-xl"
          >
            {entry.items.map((child, i) => (
              <Entry key={child.id} entry={child} index={i} />
            ))}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
      );
    }
    case "check":
      return (
        <DropdownMenuCheckboxItem
          checked={entry.checked}
          disabled={entry.disabled}
          onSelect={(e) => {
            if (entry.keepOpen) e.preventDefault();
            entry.onSelect();
          }}
          {...stagger(index)}
        >
          {entry.color && <span aria-hidden className="size-2.5 shrink-0 rounded-full" style={{ background: entry.color }} />}
          <span className="truncate">{entry.label}</span>
        </DropdownMenuCheckboxItem>
      );
    default: {
      const Icon = entry.icon;
      return (
        <DropdownMenuItem
          variant={entry.danger ? "destructive" : "default"}
          disabled={entry.disabled}
          onSelect={(e) => {
            if (entry.keepOpen) e.preventDefault();
            entry.onSelect();
          }}
          className={cn("group/item", entry.danger && "font-bold")}
          {...stagger(index)}
        >
          {Icon && <Icon className="transition-transform duration-200 group-data-[highlighted]/item:scale-110" />}
          <span className="truncate">{entry.label}</span>
          {entry.hint && <span className="ml-auto max-w-[45%] truncate pl-2 text-xs text-muted-foreground">{entry.hint}</span>}
        </DropdownMenuItem>
      );
    }
  }
}
