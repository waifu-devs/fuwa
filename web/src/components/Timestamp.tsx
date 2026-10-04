import { autoUpdate, flip, FloatingPortal, offset, shift, useDismiss, useFloating, useInteractions } from "@floating-ui/react";
import { CalendarClockIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { formatFull, formatRelative, formatTimestamp, parseToken, refreshEvery, type TimestampStyle } from "@/lib/timestamps";

/*
 * Timestamps in Markdown: `<t:1759544400:R>` reads "in 3 hours" for whoever
 * sees it, in their own time zone and language. `remarkTimestamps` (in
 * lib/timestamps) finds the tokens and this draws the `fuwa-time` elements.
 * Hovering one, or pressing it on a phone, shows the whole date.
 */

// ───────────────────────── One clock for every relative timestamp ─────────────────────────

/*
 * Relative timestamps share one timer, which runs only while one is on
 * screen and ticks as often as the soonest of them needs ("in 5 seconds"
 * every second, "in 3 hours" every minute). Each one re-renders only when its
 * own words change.
 */
const watchers = new Map<() => void, number>();
let now = Date.now();
let timer: ReturnType<typeof setTimeout> | undefined;

function schedule() {
  clearTimeout(timer);
  timer = undefined;
  if (!watchers.size) return;
  let wait = 60_000;
  for (const ms of watchers.values()) wait = Math.min(wait, refreshEvery(ms, now));
  // On the edge of the second (or minute), so the count steps in time with the clock.
  timer = setTimeout(tick, wait - (Date.now() % wait) + 5);
}

function tick() {
  now = Date.now();
  for (const notify of watchers.keys()) notify();
  schedule();
}

function watch(ms: number, notify: () => void) {
  const first = !watchers.size;
  watchers.set(notify, ms);
  if (first) now = Date.now();
  schedule();
  if (first) document.addEventListener("visibilitychange", onVisible);
  return () => {
    watchers.delete(notify);
    if (!watchers.size) document.removeEventListener("visibilitychange", onVisible);
    schedule();
  };
}

// A tab coming back from the background catches up at once.
const onVisible = () => document.visibilityState === "visible" && tick();

function useRelative(ms: number) {
  const subscribe = useCallback((notify: () => void) => watch(ms, notify), [ms]);
  return useSyncExternalStore(subscribe, () => formatRelative(ms, now));
}

// ───────────────────────── Drawing them ─────────────────────────

type TimeProps = { children?: ReactNode; "data-time"?: string; "data-style"?: string };

const pill =
  "timestamp inline rounded-md bg-muted px-1 py-px font-medium whitespace-nowrap text-foreground/90 transition-colors duration-150 hover:bg-primary/15 hover:text-primary";

/** A timestamp from a message. Anything that isn't a real one stays as typed. */
export function Timestamp(props: TimeProps) {
  const parsed = parseToken(props["data-time"] ?? "", props["data-style"] || undefined);
  if (!parsed) return <>{props.children}</>;
  return parsed.style === "R" ? <RelativeTime ms={parsed.ms} /> : <AbsoluteTime ms={parsed.ms} style={parsed.style} />;
}

function AbsoluteTime({ ms, style }: { ms: number; style: TimestampStyle }) {
  const text = useMemo(() => formatTimestamp(ms, style, 0), [ms, style]);
  return <TimeChip ms={ms}>{text}</TimeChip>;
}

function RelativeTime({ ms }: { ms: number }) {
  return <TimeChip ms={ms}>{useRelative(ms)}</TimeChip>;
}

/**
 * The chip itself. The card with the whole date is only made while it shows,
 * so a channel full of timestamps costs no more than one full of words.
 */
export function TimeChip({ ms, children }: { ms: number; children: ReactNode }) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const press = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const touched = useRef(false);
  const pointer = useRef("");
  const iso = useMemo(() => new Date(ms).toISOString(), [ms]);

  return (
    <>
      <time
        dateTime={iso}
        className={pill}
        role="button"
        tabIndex={0}
        onFocus={(e) => e.currentTarget.matches(":focus-visible") && setAnchor(e.currentTarget)}
        onBlur={() => setAnchor(null)}
        onKeyDown={(e) => {
          if (e.key !== "Enter" && e.key !== " ") return;
          e.preventDefault();
          const el = e.currentTarget;
          setAnchor((open) => (open ? null : el));
        }}
        onPointerEnter={(e) => e.pointerType === "mouse" && setAnchor(e.currentTarget)}
        onPointerLeave={(e) => e.pointerType === "mouse" && setAnchor(null)}
        onPointerDown={(e) => {
          pointer.current = e.pointerType;
          if (e.pointerType === "mouse") return;
          touched.current = false;
          const el = e.currentTarget;
          // A long press opens it, as does a tap.
          press.current = setTimeout(() => {
            touched.current = true;
            setAnchor(el);
          }, 400);
        }}
        onPointerUp={() => clearTimeout(press.current)}
        onPointerCancel={() => clearTimeout(press.current)}
        onClick={(e) => {
          // A mouse already opened it by hovering.
          if (touched.current || pointer.current === "mouse") return;
          const el = e.currentTarget;
          setAnchor((open) => (open ? null : el));
        }}
        onContextMenu={(e) => touched.current && e.preventDefault()}
      >
        {children}
      </time>
      <AnimatePresence>{anchor && <FullDate key="full" anchor={anchor} ms={ms} onClose={() => setAnchor(null)} />}</AnimatePresence>
    </>
  );
}

function FullDate({ anchor, ms, onClose }: { anchor: HTMLElement; ms: number; onClose: () => void }) {
  const { refs, floatingStyles, context } = useFloating({
    open: true,
    onOpenChange: (open) => !open && onClose(),
    elements: { reference: anchor },
    placement: "top",
    whileElementsMounted: autoUpdate,
    middleware: [offset(6), flip(), shift({ padding: 8 })],
  });
  const { getFloatingProps } = useInteractions([useDismiss(context, { ancestorScroll: true })]);
  const full = useMemo(() => formatFull(ms), [ms]);
  const zone = useMemo(() => new Intl.DateTimeFormat(undefined, { timeZoneName: "long" }).formatToParts(ms).find((p) => p.type === "timeZoneName")?.value, [ms]);
  return (
    <FloatingPortal>
      <div ref={refs.setFloating} style={floatingStyles} className="pointer-events-none z-50" {...getFloatingProps()}>
        <motion.div
          role="tooltip"
          initial={{ opacity: 0, scale: 0.9, y: 4 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.95, y: 2 }}
          transition={{ type: "spring", stiffness: 700, damping: 32 }}
          className="flex max-w-[calc(100vw-1rem)] items-center gap-2 rounded-xl border bg-popover px-3 py-2 text-sm shadow-xl"
        >
          <CalendarClockIcon className="size-4 shrink-0 text-primary" />
          <span className="min-w-0">
            <span className="block font-bold">{full}</span>
            {zone && <span className="block text-xs text-muted-foreground">{zone}</span>}
          </span>
        </motion.div>
      </div>
    </FloatingPortal>
  );
}
