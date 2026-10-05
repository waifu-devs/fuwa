import { AlertCircleIcon, LoaderCircleIcon, PauseIcon, PlayIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { memo, useEffect, useMemo, useRef, type KeyboardEvent, type PointerEvent } from "react";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { useI18n } from "@/i18n/react";
import { clock, nextRate, onPosition, positionOf, seek, toggle, useVoice, type Loader } from "@/voice/player";
import { heights } from "@/voice/waveform";

/** How many bars a message's waveform is drawn with. */
const BARS = 40;

/**
 * A voice message: play and pause, its waveform (which is also where you
 * scrub), how long it is, and how fast it plays. The sound itself is
 * fetched and opened only when it's first played. Where it's playing comes
 * from the page's one player (voice/player.ts), so it carries on while the
 * row scrolls away and shows where it's got to when it scrolls back; moving
 * along sets transforms on the bars, never re-rendering the row.
 *
 * `id` names it for the player: unique on the page. Nothing here knows
 * where the sound comes from (`load`), so server channels can use it too.
 */
export const VoiceMessage = memo(function VoiceMessage({
  id,
  durationMs,
  waveform,
  load,
  pending = false,
  className,
}: {
  id: string;
  durationMs: number;
  waveform: Uint8Array | undefined;
  load: Loader | null;
  /** Still on its way: drawn, but not playable. */
  pending?: boolean;
  className?: string;
}) {
  const { current, failed, rate } = useVoice(id);
  const { t } = useI18n();
  const bars = useMemo(() => heights(waveform, BARS), [waveform]);
  const reveal = useRef<HTMLDivElement>(null);
  const unreveal = useRef<HTMLDivElement>(null);
  const time = useRef<HTMLSpanElement>(null);
  const track = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  // Shows where it is: the played bars uncovered from the left, by transforms only.
  const show = (ms: number) => {
    const f = durationMs > 0 ? Math.max(0, Math.min(1, ms / durationMs)) : 0;
    if (reveal.current) reveal.current.style.transform = `translateX(${(f - 1) * 100}%)`;
    if (unreveal.current) unreveal.current.style.transform = `translateX(${(1 - f) * 100}%)`;
    if (time.current) time.current.textContent = ms > 0 ? clock(ms) : clock(durationMs);
    track.current?.setAttribute("aria-valuenow", String(Math.round(ms / 1000)));
  };

  useEffect(() => {
    show(positionOf(id));
    if (!current) return;
    return onPosition((_, ms) => {
      if (!dragging.current) show(ms);
    });
    // `show` reads only refs and durationMs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, id, durationMs]);

  const usable = !pending && !!load && !failed;

  const fractionAt = (e: PointerEvent<HTMLDivElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    return box.width ? (e.clientX - box.left) / box.width : 0;
  };
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (!usable || e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    dragging.current = true;
    show(fractionAt(e) * durationMs);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!dragging.current) return;
    show(Math.max(0, Math.min(1, fractionAt(e))) * durationMs);
  };
  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    if (!dragging.current || !load) return;
    dragging.current = false;
    seek(id, Math.max(0, Math.min(1, fractionAt(e))), durationMs, load);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!usable || !load) return;
    const step = e.key === "ArrowRight" ? 5000 : e.key === "ArrowLeft" ? -5000 : e.key === "Home" ? -Infinity : e.key === "End" ? Infinity : 0;
    if (!step) return;
    e.preventDefault();
    const ms = Math.max(0, Math.min(durationMs, positionOf(id) + step));
    seek(id, durationMs ? ms / durationMs : 0, durationMs, load);
  };

  return (
    <div
      className={cn(
        "voice-message mt-1 flex w-full max-w-[22rem] items-center gap-2.5 rounded-2xl border bg-card/70 py-1.5 pr-2 pl-1.5 shadow-sm",
        className,
      )}
    >
      <PlayButton id={id} load={load} usable={usable} />
      <div
        ref={track}
        role="slider"
        tabIndex={usable ? 0 : -1}
        aria-label={t("dms-calls.voice.message.position")}
        aria-valuemin={0}
        aria-valuemax={Math.round(durationMs / 1000)}
        aria-valuenow={0}
        aria-disabled={!usable}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={() => (dragging.current = false)}
        onKeyDown={onKeyDown}
        className={cn(
          "relative h-8 min-w-0 flex-1 touch-none rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring",
          usable && "cursor-pointer",
        )}
      >
        <Bars bars={bars} className="text-foreground/20" />
        <div className="absolute inset-0 overflow-hidden">
          <div ref={reveal} className="absolute inset-0 overflow-hidden" style={{ transform: "translateX(-100%)" }}>
            <div ref={unreveal} className="absolute inset-0" style={{ transform: "translateX(100%)" }}>
              <Bars bars={bars} className="text-primary" />
            </div>
          </div>
        </div>
      </div>
      <span ref={time} className="w-9 shrink-0 text-right text-xs font-bold tabular-nums text-muted-foreground">
        {clock(durationMs)}
      </span>
      <RateButton rate={rate} />
      {failed && (
        <span className="sr-only" role="status">
          {failed}
        </span>
      )}
    </div>
  );
});

/** Play and pause, turning into a spinner while it loads and a warning when it can't. */
function PlayButton({ id, load, usable }: { id: string; load: Loader | null; usable: boolean }) {
  const { playing, loading, failed } = useVoice(id);
  const { t } = useI18n();
  return (
    <motion.button
      type="button"
      disabled={!usable}
      onClick={() => load && void toggle(id, load)}
      whileTap={usable ? { scale: 0.88 } : undefined}
      transition={SPRING}
      aria-label={playing ? t("dms-calls.voice.message.pause") : t("dms-calls.voice.message.play")}
      className={cn(
        "grid size-9 shrink-0 place-items-center rounded-full transition-colors",
        usable ? "bg-primary text-primary-foreground shadow-[0_6px_16px_-8px_var(--primary)]" : "bg-muted text-muted-foreground",
      )}
    >
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={failed ? "failed" : loading ? "loading" : playing ? "pause" : "play"}
          initial={{ scale: 0.4, opacity: 0, rotate: -30 }}
          animate={{ scale: 1, opacity: 1, rotate: 0 }}
          exit={{ scale: 0.4, opacity: 0, rotate: 30 }}
          transition={{ type: "spring", stiffness: 700, damping: 28 }}
          className="grid place-items-center"
        >
          {failed ? (
            <AlertCircleIcon className="size-[18px]" />
          ) : loading ? (
            <LoaderCircleIcon className="size-[18px] animate-spin" />
          ) : playing ? (
            <PauseIcon className="size-[18px] fill-current" />
          ) : (
            <PlayIcon className="size-[18px] translate-x-px fill-current" />
          )}
        </motion.span>
      </AnimatePresence>
    </motion.button>
  );
}

/** How fast voice messages play; each press steps to the next speed. */
function RateButton({ rate }: { rate: number }) {
  const { t } = useI18n();
  return (
    <motion.button
      type="button"
      onClick={nextRate}
      whileTap={{ scale: 0.88 }}
      aria-label={t("dms-calls.voice.message.speed", { rate })}
      title={t("dms-calls.voice.message.speedTitle")}
      className="h-6 w-10 shrink-0 overflow-hidden rounded-full bg-muted text-[0.7rem] font-bold tabular-nums text-muted-foreground transition-colors hover:bg-muted/70 hover:text-foreground"
    >
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={rate}
          initial={{ y: 12, opacity: 0 }}
          animate={{ y: 0, opacity: 1 }}
          exit={{ y: -12, opacity: 0 }}
          transition={SPRING}
          className="block"
        >
          {t("dms-calls.voice.message.rate", { rate })}
        </motion.span>
      </AnimatePresence>
    </motion.button>
  );
}

/** A waveform's bars, as tall as they're loud, centred. */
function Bars({ bars, className }: { bars: number[]; className?: string }) {
  return (
    <div className={cn("absolute inset-0 flex items-center gap-[2px]", className)} aria-hidden>
      {bars.map((h, i) => (
        <span
          // Bars never move or reorder.
          // eslint-disable-next-line react/no-array-index-key
          key={i}
          className="h-full min-w-[2px] flex-1 origin-center rounded-full bg-current"
          style={{ transform: `scaleY(${Math.max(0.12, h)})` }}
        />
      ))}
    </div>
  );
}

/** Why a voice message can't be played, under it. */
export function VoiceProblem({ id }: { id: string }) {
  const { failed } = useVoice(id);
  return (
    <AnimatePresence>
      {failed && (
        <motion.p
          initial={{ opacity: 0, y: -4 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0 }}
          transition={SPRING}
          className="mt-1 text-xs text-destructive"
        >
          {failed}
        </motion.p>
      )}
    </AnimatePresence>
  );
}
