import {
  autoUpdate,
  flip,
  FloatingFocusManager,
  FloatingPortal,
  offset,
  shift,
  useClick,
  useDismiss,
  useFloating,
  useInteractions,
  useRole,
} from "@floating-ui/react";
import { CalendarClockIcon, CheckIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useId, useMemo, useState } from "react";
import { SPRING } from "@/components/motion";
import { type Key, useI18n } from "@/i18n/react";
import { useNow } from "@/lib/notifications";
import { actionById, bindingOf, comboLabel } from "@/lib/keybinds";
import { usePrefs } from "@/lib/prefs";
import { formatTimestamp, STYLES, toToken, type TimestampStyle } from "@/lib/timestamps";
import { onCommand } from "@/lib/ui";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

/*
 * The composer's timestamp button: pick a day, a time and how it should
 * read, see it as others will, and put the token (`<t:1759594800:R>`) at the
 * caret. It's plain text, so it works anywhere you can write, encrypted or
 * not, and reads the same in Discord. The "Insert a timestamp" shortcut
 * (Alt+Shift+T unless you change it) opens it from the message box.
 */

/** Each style's name in the catalog. */
const STYLE_NAMES: Record<TimestampStyle, Key> = {
  t: "chattools.timestamp.style.shortTime",
  T: "chattools.timestamp.style.longTime",
  d: "chattools.timestamp.style.shortDate",
  D: "chattools.timestamp.style.longDate",
  f: "chattools.timestamp.style.dateTime",
  F: "chattools.timestamp.style.dayDateTime",
  R: "chattools.timestamp.style.relative",
};

/** The style you picked last, for next time. */
let lastStyle: TimestampStyle = "R";

const pad = (n: number) => String(n).padStart(2, "0");
const dateValue = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const timeValue = (d: Date) => `${pad(d.getHours())}:${pad(d.getMinutes())}`;

/** The moment the two fields name, on this device's clock; null while one is empty. */
function fromFields(date: string, time: string): Date | null {
  const d = /^(\d{4,6})-(\d{2})-(\d{2})$/.exec(date);
  const t = /^(\d{2}):(\d{2})/.exec(time);
  if (!d || !t) return null;
  const at = new Date(Number(d[1]), Number(d[2]) - 1, Number(d[3]), Number(t[1]), Number(t[2]));
  return Number.isNaN(at.getTime()) ? null : at;
}

/** The top of the next hour, where the picker starts. */
function nextHour(now: number) {
  const d = new Date(now);
  d.setHours(d.getHours() + 1, 0, 0, 0);
  return d;
}

/** One-tap times. */
function quickPicks(now: number): { label: Key; at: Date }[] {
  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  tomorrow.setHours(9, 0, 0, 0);
  const tonight = new Date(now);
  tonight.setHours(20, 0, 0, 0);
  const week = new Date(now);
  week.setDate(week.getDate() + 7);
  week.setSeconds(0, 0);
  const hour = new Date(now + 3_600_000);
  hour.setSeconds(0, 0);
  const picks = [
    { label: "chattools.timestamp.inAnHour" as const, at: hour },
    { label: "chattools.timestamp.tonight" as const, at: tonight },
    { label: "chattools.timestamp.tomorrowMorning" as const, at: tomorrow },
    { label: "chattools.timestamp.inAWeek" as const, at: week },
  ];
  return picks.filter((p) => p.at.getTime() > now);
}

export function TimestampPicker({ onPick }: { onPick: (token: string) => void }) {
  const [open, setOpen] = useState(false);
  const { refs, floatingStyles, context } = useFloating({
    open,
    onOpenChange: setOpen,
    placement: "top-end",
    whileElementsMounted: autoUpdate,
    middleware: [offset(8), flip(), shift({ padding: 8 })],
  });
  const { getReferenceProps, getFloatingProps } = useInteractions([useClick(context), useDismiss(context), useRole(context, { role: "dialog" })]);
  const combo = usePrefs((p) => {
    const action = actionById("insertTimestamp");
    return action ? bindingOf(action, p) : null;
  });

  const { t } = useI18n();

  // On a phone, focusing the date field would throw up the keyboard over the picker.
  const coarse = useMediaQuery("(pointer: coarse)");

  useEffect(() => onCommand("insertTimestamp", () => setOpen((o) => !o)), []);

  return (
    <>
      <motion.button
        ref={refs.setReference}
        type="button"
        aria-label={t("chattools.timestamp.insert")}
        title={combo ? t("chattools.timestamp.insertWithKeys", { keys: comboLabel(combo) }) : t("chattools.timestamp.insert")}
        whileHover={{ scale: 1.12, rotate: 8 }}
        whileTap={{ scale: 0.85 }}
        className={cn(
          "mb-0.5 grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition-colors hover:text-primary",
          open && "bg-primary/10 text-primary",
        )}
        {...getReferenceProps()}
      >
        <CalendarClockIcon className="size-[18px]" />
      </motion.button>
      <FloatingPortal>
        <AnimatePresence>
          {open && (
            <FloatingFocusManager context={context} initialFocus={coarse ? -1 : 0} modal={false} returnFocus={false}>
              <div ref={refs.setFloating} style={floatingStyles} className="z-50" {...getFloatingProps()}>
                <Panel
                  onPick={(token) => {
                    setOpen(false);
                    onPick(token);
                  }}
                />
              </div>
            </FloatingFocusManager>
          )}
        </AnimatePresence>
      </FloatingPortal>
    </>
  );
}

function Panel({ onPick }: { onPick: (token: string) => void }) {
  const now = useNow(1000);
  const [opened] = useState(Date.now);
  const start = useMemo(() => nextHour(opened), [opened]);
  const [date, setDate] = useState(() => dateValue(start));
  const [time, setTime] = useState(() => timeValue(start));
  const [style, setStyle] = useState<TimestampStyle>(lastStyle);
  const id = useId();
  const at = fromFields(date, time);
  const token = at ? toToken(at, style) : "";
  const picks = useMemo(() => quickPicks(opened), [opened]);
  const { t } = useI18n();

  function choose(next: TimestampStyle) {
    setStyle(next);
    lastStyle = next;
  }
  function insert() {
    if (token) onPick(token);
  }

  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.92, y: 8 }}
      animate={{ opacity: 1, scale: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.95, y: 6 }}
      transition={SPRING}
      style={{ transformOrigin: "bottom right" }}
      className="flex w-[21rem] max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-2xl border bg-popover shadow-2xl"
      onKeyDown={(e) => {
        if (e.key === "Enter" && !e.nativeEvent.isComposing) {
          e.preventDefault();
          insert();
        }
      }}
    >
      <div className="border-b px-3 pt-3 pb-2.5">
        <p className="text-sm font-extrabold">{t("chattools.timestamp.insert")}</p>
        <p className="text-xs text-muted-foreground">{t("chattools.timestamp.about")}</p>
        <div className="mt-2.5 grid grid-cols-[1fr_auto] gap-2">
          <label className="sr-only" htmlFor={`${id}-date`}>
            {t("chattools.timestamp.date")}
          </label>
          <input
            id={`${id}-date`}
            type="date"
            value={date}
            onChange={(e) => setDate(e.target.value)}
            className="h-9 min-w-0 rounded-xl bg-muted/60 px-2.5 text-sm outline-none focus:ring-2 focus:ring-primary/40"
          />
          <label className="sr-only" htmlFor={`${id}-time`}>
            {t("chattools.timestamp.time")}
          </label>
          <input
            id={`${id}-time`}
            type="time"
            value={time}
            onChange={(e) => setTime(e.target.value)}
            className="h-9 rounded-xl bg-muted/60 px-2.5 text-sm outline-none focus:ring-2 focus:ring-primary/40"
          />
        </div>
        <div className="scroll-thin mt-2 flex gap-1.5 overflow-x-auto pb-0.5">
          {picks.map((p, n) => {
            const on = date === dateValue(p.at) && time === timeValue(p.at);
            return (
              <motion.button
                key={p.label}
                type="button"
                initial={{ opacity: 0, y: 4 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ ...SPRING, delay: 0.04 + n * 0.03 }}
                whileTap={{ scale: 0.92 }}
                onClick={() => {
                  setDate(dateValue(p.at));
                  setTime(timeValue(p.at));
                }}
                className={cn(
                  "shrink-0 rounded-full border px-2.5 py-1 text-xs font-bold whitespace-nowrap transition-colors",
                  on ? "border-primary/50 bg-primary/15 text-primary" : "text-muted-foreground hover:bg-muted hover:text-foreground",
                )}
              >
                {t(p.label)}
              </motion.button>
            );
          })}
        </div>
      </div>
      <div role="radiogroup" aria-label={t("chattools.timestamp.howItReads")} className="p-1.5">
        {STYLES.map((s, n) => {
          const on = s.style === style;
          return (
            <motion.button
              key={s.style}
              type="button"
              role="radio"
              aria-checked={on}
              initial={{ opacity: 0, x: -6 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ ...SPRING, delay: 0.06 + n * 0.025 }}
              onClick={() => choose(s.style)}
              onDoubleClick={() => {
                choose(s.style);
                if (at) onPick(toToken(at, s.style));
              }}
              className="relative flex w-full items-center gap-2 rounded-xl px-2.5 py-1.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
            >
              {on && <motion.span layoutId="timestamp-style" transition={SPRING} className="absolute inset-0 rounded-xl bg-primary/12" />}
              <span className="relative min-w-0 flex-1">
                <span className={cn("block truncate text-sm font-bold", on && "text-primary")}>
                  {at ? formatTimestamp(at.getTime(), s.style, now) : t("chattools.timestamp.pickDateTime")}
                </span>
                <span className="block text-[0.7rem] text-muted-foreground">{t(STYLE_NAMES[s.style])}</span>
              </span>
              <AnimatePresence initial={false}>
                {on && (
                  <motion.span
                    initial={{ scale: 0, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    exit={{ scale: 0, opacity: 0 }}
                    transition={{ type: "spring", stiffness: 700, damping: 24 }}
                    className="relative text-primary"
                  >
                    <CheckIcon className="size-4" />
                  </motion.span>
                )}
              </AnimatePresence>
            </motion.button>
          );
        })}
      </div>
      <div className="flex items-center gap-2 border-t bg-muted/30 px-3 py-2">
        <code className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground" title={t("chattools.timestamp.sent")}>
          {token || " "}
        </code>
        <motion.button
          type="button"
          onClick={insert}
          disabled={!token}
          whileTap={{ scale: 0.92 }}
          className="h-8 shrink-0 rounded-xl bg-primary px-3.5 text-xs font-bold text-primary-foreground shadow-[0_6px_18px_-8px_var(--primary)] transition-opacity disabled:opacity-50"
        >
          {t("chattools.timestamp.insertButton")}
        </motion.button>
      </div>
    </motion.div>
  );
}
