import { MicIcon, SendHorizontalIcon, Trash2Icon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type PointerEvent } from "react";
import { micProblem } from "@/calls/audio";
import { SPRING } from "@/components/motion";
import { reduceMotion } from "@/lib/prefs";
import { reportError } from "@/lib/reports";
import { cn } from "@/lib/utils";
import { useI18n } from "@/i18n/react";
import { pauseAll } from "@/voice/player";
import { canRecord, MIN_MS, Recorder, type Clip } from "@/voice/recorder";
import { clock } from "@/voice/player";

/** Held this long, the mic records only while held; a quicker tap starts and stops it. */
const HOLD_MS = 280;
/** Dragged this far left while holding, the recording is thrown away. */
const CANCEL_PX = 96;
/** Bars of the live waveform. */
const LIVE_BARS = 64;

type Mode = "idle" | "starting" | "hold" | "tap";

/**
 * The microphone button in a composer, and the recording bar it opens over
 * the composer. Hold it to record and let go to send, sliding left to throw
 * it away; or tap it to start, then tap send (or Escape to throw it away).
 * The bar shows a red dot, the time and the live waveform; everything in it
 * moves by transforms. Recording stops by itself at the instance's longest
 * voice message (`maxMs`, 0 for none).
 *
 * It knows nothing about where the recording goes (`onSend`), so a server
 * channel's composer can use it too. It must sit inside the composer's
 * positioned box: the bar covers that box.
 */
export function VoiceRecorder({
  maxMs,
  onSend,
  onProblem,
  disabled = false,
}: {
  maxMs: () => Promise<number>;
  onSend: (clip: Clip) => void;
  onProblem: (problem: string | null) => void;
  disabled?: boolean;
}) {
  const [mode, setMode] = useState<Mode>("idle");
  const [limited, setLimited] = useState(false);
  const [dragX, setDragX] = useState(0);
  const recorder = useRef<Recorder | null>(null);
  const pressedAt = useRef(0);
  const startX = useRef(0);
  const holding = useRef(false);
  const bars = useRef<(HTMLSpanElement | null)[]>([]);
  const time = useRef<HTMLSpanElement>(null);
  const frame = useRef(0);
  const { t } = useI18n();

  const supported = canRecord();

  const stopDrawing = () => cancelAnimationFrame(frame.current);
  const draw = useCallback(() => {
    const r = recorder.current;
    if (!r) return;
    const levels = r.recentLevels;
    // Two levels a bar, newest at the right.
    for (let i = 0; i < LIVE_BARS; i++) {
      const at = levels.length - (LIVE_BARS - i) * 2;
      const level = at >= 0 ? Math.max(levels[at] ?? 0, levels[at + 1] ?? 0) : 0;
      const el = bars.current[i];
      if (el) el.style.transform = `scaleY(${Math.max(0.1, Math.min(1, Math.sqrt(level) * 1.4))})`;
    }
    if (time.current) time.current.textContent = clock(r.elapsedMs);
    frame.current = requestAnimationFrame(draw);
  }, []);

  const reset = useCallback(() => {
    stopDrawing();
    recorder.current = null;
    holding.current = false;
    setDragX(0);
    setLimited(false);
    setMode("idle");
  }, []);

  const cancel = useCallback(() => {
    recorder.current?.cancel();
    reset();
  }, [reset]);

  const finish = useCallback(async () => {
    const r = recorder.current;
    if (!r) return;
    recorder.current = null;
    stopDrawing();
    try {
      const clip = await r.stop();
      if (clip.durationMs < MIN_MS) {
        onProblem(t("dms-calls.voice.recorder.tooShort"));
      } else {
        onSend(clip);
      }
    } catch {
      onProblem(t("dms-calls.voice.recorder.notSaved"));
    }
    reset();
  }, [onProblem, onSend, reset, t]);

  const start = async (as: Mode) => {
    if (recorder.current || mode === "starting") return;
    onProblem(null);
    pauseAll();
    setMode("starting");
    try {
      const max = await maxMs();
      const r = await Recorder.start(max, { onLimit: () => setLimited(true) });
      // Gone while the microphone opened (the conversation changed): let it go.
      if (!mounted.current) return r.cancel();
      recorder.current = r;
      setMode(holding.current ? "hold" : as === "hold" ? "tap" : as);
      frame.current = requestAnimationFrame(draw);
    } catch (err) {
      if (!(err instanceof DOMException && err.name === "NotSupportedError")) reportError("voice_mic", "voice.record");
      onProblem(err instanceof DOMException && err.name === "NotSupportedError" ? t("dms-calls.voice.recorder.unsupported") : micProblem(err));
      reset();
    }
  };

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      recorder.current?.cancel();
      cancelAnimationFrame(frame.current);
    };
  }, []);

  // Escape throws a recording away; Enter sends one started with a tap.
  const keys = useRef({ cancel, finish });
  useLayoutEffect(() => {
    keys.current = { cancel, finish };
  });
  const listening = mode === "tap" || mode === "hold";
  const tapped = mode === "tap";
  useEffect(() => {
    if (!listening) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        keys.current.cancel();
      } else if (e.key === "Enter" && tapped) {
        e.preventDefault();
        void keys.current.finish();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [listening, tapped]);

  const onPointerDown = (e: PointerEvent<HTMLButtonElement>) => {
    if (disabled || !supported || e.button !== 0) return;
    if (mode === "tap") return;
    e.currentTarget.setPointerCapture(e.pointerId);
    pressedAt.current = performance.now();
    startX.current = e.clientX;
    holding.current = true;
    void start("hold");
  };
  const onPointerMove = (e: PointerEvent<HTMLButtonElement>) => {
    if (!holding.current) return;
    const dx = Math.min(0, e.clientX - startX.current);
    setDragX(dx);
    if (dx < -CANCEL_PX) cancel();
  };
  const onPointerUp = () => {
    if (!holding.current) return;
    holding.current = false;
    const held = performance.now() - pressedAt.current;
    if (held < HOLD_MS) {
      // A tap: keep recording until send or cancel.
      setMode((m) => (m === "hold" ? "tap" : m));
      setDragX(0);
      return;
    }
    void finish();
  };

  const recording = mode === "hold" || mode === "tap" || mode === "starting";
  const cancelling = Math.min(1, -dragX / CANCEL_PX);
  const motionOk = !reduceMotion();

  if (!supported) {
    return (
      <button
        type="button"
        disabled
        aria-label={t("dms-calls.voice.recorder.unsupportedLabel")}
        title={t("dms-calls.voice.recorder.unsupportedLabel")}
        className="relative z-20 mb-0.5 grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground/50"
      >
        <MicIcon className="size-[18px]" />
      </button>
    );
  }

  return (
    <>
      <AnimatePresence>
        {recording && (
          <motion.div
            key="bar"
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 8 }}
            transition={SPRING}
            className="absolute inset-0 z-10 flex items-center gap-3 rounded-2xl bg-card pr-14 pl-2"
            role="status"
            aria-label={t("dms-calls.voice.recorder.recording")}
          >
            <motion.button
              type="button"
              onClick={cancel}
              whileTap={{ scale: 0.85 }}
              aria-label={t("dms-calls.voice.recorder.throwAway")}
              title={t("dms-calls.voice.recorder.throwAwayTitle")}
              className="grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
            >
              <Trash2Icon className="size-[18px]" />
            </motion.button>
            <span className="relative grid size-3 shrink-0 place-items-center">
              <motion.span
                className="absolute inset-0 rounded-full bg-destructive"
                animate={limited || !motionOk ? { scale: 1, opacity: 1 } : { scale: [1, 1.9, 1], opacity: [0.6, 0, 0.6] }}
                transition={{ duration: 1.4, repeat: Infinity, ease: "easeOut" }}
              />
              <span className="relative size-2.5 rounded-full bg-destructive" />
            </span>
            <span ref={time} className="w-10 shrink-0 text-sm font-bold tabular-nums">
              0:00
            </span>
            <div
              className="relative flex h-7 min-w-0 flex-1 items-center justify-end gap-[3px] overflow-hidden text-primary"
              style={{ opacity: 1 - cancelling * 0.7 }}
              aria-hidden
            >
              {Array.from({ length: LIVE_BARS }, (_, i) => (
                <span
                  // Fixed slots the live levels flow through.
                  // eslint-disable-next-line react/no-array-index-key
                  key={i}
                  ref={(el) => {
                    bars.current[i] = el;
                  }}
                  className="h-full w-[3px] shrink-0 origin-center rounded-full bg-current transition-transform duration-75"
                  style={{ transform: "scaleY(0.1)" }}
                />
              ))}
            </div>
            <span
              className={cn("hidden shrink-0 text-xs font-bold text-muted-foreground sm:block", limited && "text-amber-600 dark:text-amber-400")}
              style={{ transform: `translateX(${dragX * 0.4}px)`, opacity: mode === "hold" ? 1 - cancelling : 1 }}
            >
              {limited ? t("dms-calls.voice.recorder.longest") : mode === "hold" ? t("dms-calls.voice.recorder.slide") : t("dms-calls.voice.recorder.esc")}
            </span>
          </motion.div>
        )}
      </AnimatePresence>
      <motion.button
        type="button"
        disabled={disabled}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={() => holding.current && cancel()}
        onClick={() => {
          if (mode === "tap") void finish();
        }}
        onKeyDown={(e) => {
          // From the keyboard, Space or Enter starts a recording like a tap.
          if ((e.key === " " || e.key === "Enter") && mode === "idle") {
            e.preventDefault();
            void start("tap");
          }
        }}
        animate={{ scale: mode === "hold" ? 1.12 : 1, x: dragX }}
        transition={dragX ? { x: { duration: 0 }, scale: SPRING } : SPRING}
        whileTap={{ scale: 0.9 }}
        aria-label={mode === "tap" ? t("dms-calls.voice.recorder.send") : t("dms-calls.voice.recorder.record")}
        title={mode === "tap" ? t("dms-calls.voice.recorder.sendTitle") : t("dms-calls.voice.recorder.recordTitle")}
        className={cn(
          "relative z-20 mb-0.5 grid size-9 shrink-0 touch-none place-items-center rounded-xl transition-colors select-none",
          recording
            ? "bg-primary text-primary-foreground shadow-[0_6px_18px_-8px_var(--primary)]"
            : "text-muted-foreground hover:bg-muted hover:text-foreground",
        )}
      >
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span
            key={mode === "tap" ? "send" : "mic"}
            initial={{ scale: 0.4, opacity: 0, rotate: -25 }}
            animate={{ scale: 1, opacity: 1, rotate: 0 }}
            exit={{ scale: 0.4, opacity: 0, rotate: 25 }}
            transition={{ type: "spring", stiffness: 700, damping: 26 }}
            className="grid place-items-center"
          >
            {mode === "tap" ? <SendHorizontalIcon className="size-[18px]" /> : <MicIcon className="size-[18px]" />}
          </motion.span>
        </AnimatePresence>
      </motion.button>
    </>
  );
}
