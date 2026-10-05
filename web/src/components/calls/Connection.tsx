import * as Popover from "@radix-ui/react-popover";
import { AnimatePresence, m as motion } from "motion/react";
import { useCallback, useId, useRef, useState, type ReactNode } from "react";
import type { ActiveCall } from "@/calls/state";
import { formatPing, useQualityEffect, useQualityLevel, useQualityText, type Level, type Quality, type Route } from "@/calls/quality";
import { SwapText } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { type Key, useI18n } from "@/i18n/react";

const COLOR: Record<Level, string> = { good: "#3ba55d", okay: "#f0b232", poor: "#ed4245" };
const LIT: Record<Level, number> = { good: 3, okay: 2, poor: 1 };
const LABEL: Record<Level, Key> = { good: "dms-calls.calls.connection.good", okay: "dms-calls.calls.connection.okay", poor: "dms-calls.calls.connection.poor" };
const ROUTE: Record<Route, Key> = {
  udp: "dms-calls.calls.connection.udp",
  tcp: "dms-calls.calls.connection.tcp",
  relay: "dms-calls.calls.connection.relay",
};

/**
 * Three bars: they pulse while connecting, then show how good the
 * connection is: all three green, two amber, or one red.
 */
export function Signal({ status, level }: { status: ActiveCall["status"]; level: Level | null }) {
  const ok = status === "connected";
  const lit = ok && level ? LIT[level] : 3;
  const color = !ok ? (status === "reconnecting" ? "#f59e0b" : "var(--muted-foreground)") : COLOR[level ?? "good"];
  return (
    <span aria-hidden className={cn("flex h-3.5 items-end gap-[2px]", !ok && "signal-connecting")}>
      {[5, 9, 13].map((h, n) => (
        <motion.span
          key={n}
          initial={{ scaleY: 0 }}
          animate={{ scaleY: 1, opacity: n < lit ? 1 : 0.25 }}
          transition={{ ...SPRING, delay: n * 0.06 }}
          className="w-[3px] origin-bottom rounded-full transition-colors duration-500"
          style={{ height: h, backgroundColor: color }}
        />
      ))}
    </span>
  );
}

/** Your ping, kept current without re-rendering anything around it. */
export function PingText({ className, colored }: { className?: string; colored?: boolean }) {
  const ref = useRef<HTMLSpanElement>(null);
  useQualityText(ref, formatPing);
  const level = useQualityLevel();
  const color = colored && level && level !== "good" ? COLOR[level] : undefined;
  return <span ref={ref} className={cn("tabular-nums transition-colors duration-500", className)} style={{ color }} />;
}

const W = 224;
const H = 44;

/** The last minute of pings as a line, scaled to the worst of it. */
function graph(history: number[]): { line: string; area: string } {
  if (history.length < 2) return { line: "", area: "" };
  const top = Math.max(100, ...history) * 1.15;
  const step = W / (history.length - 1);
  const points = history.map((p, i) => [i * step, H - (p / top) * H] as const);
  const line = points.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`).join(" ");
  return { line, area: `${line} L${W} ${H} L0 ${H} Z` };
}

function PingGraph({ level }: { level: Level | null }) {
  const line = useRef<SVGPathElement>(null);
  const area = useRef<SVGPathElement>(null);
  const id = `ping-${useId().replace(/[^a-zA-Z0-9-]/g, "")}`;
  const draw = useCallback((q: Quality) => {
    const g = graph(q.history);
    line.current?.setAttribute("d", g.line);
    area.current?.setAttribute("d", g.area);
  }, []);
  useQualityEffect(draw);
  const color = COLOR[level ?? "good"];
  return (
    <svg viewBox={`0 0 ${W} ${H}`} className="h-11 w-full overflow-visible" preserveAspectRatio="none" aria-hidden>
      <defs>
        <linearGradient id={id} x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor={color} stopOpacity="0.35" />
          <stop offset="1" stopColor={color} stopOpacity="0" />
        </linearGradient>
      </defs>
      <path ref={area} fill={`url(#${id})`} className="ping-graph" />
      <path ref={line} fill="none" stroke={color} strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke" className="ping-graph transition-[stroke] duration-500" />
    </svg>
  );
}

function Row({ label, value }: { label: string; value: (q: Quality) => string }) {
  const ref = useRef<HTMLSpanElement>(null);
  useQualityText(ref, value);
  return (
    <div className="flex items-center justify-between gap-3 text-xs">
      <span className="text-muted-foreground">{label}</span>
      <span ref={ref} className="font-bold tabular-nums" />
    </div>
  );
}


/** Opens a card with your ping over the last minute, packet loss, jitter and how you're connected. */
export function ConnectionDetails({ status, children }: { status: ActiveCall["status"]; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const level = useQualityLevel();
  const { t, number } = useI18n();
  const loss = useCallback(
    (q: Quality) => {
      const digits = q.loss < 0.1 ? 1 : 0;
      return q.ping === null ? "–" : number(q.loss, { style: "percent", minimumFractionDigits: digits, maximumFractionDigits: digits });
    },
    [number],
  );
  const jitter = useCallback((q: Quality) => (q.ping === null ? "–" : number(q.jitter, { style: "unit", unit: "millisecond" })), [number]);
  const route = useCallback((q: Quality) => (q.route ? t(ROUTE[q.route]) : "–"), [t]);
  const heading =
    status !== "connected"
      ? t(status === "reconnecting" ? "dms-calls.calls.status.reconnecting" : "dms-calls.calls.status.connecting")
      : level
        ? t(LABEL[level])
        : t("dms-calls.calls.connection.measuring");
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>{children}</Popover.Trigger>
      <AnimatePresence>
        {open && (
          <Popover.Portal forceMount>
            <Popover.Content asChild side="top" align="start" sideOffset={10} collisionPadding={12} forceMount>
              <motion.div
                initial={{ opacity: 0, scale: 0.92, y: 8 }}
                animate={{ opacity: 1, scale: 1, y: 0 }}
                exit={{ opacity: 0, scale: 0.95, y: 6 }}
                transition={SPRING}
                className="z-50 w-64 origin-[var(--radix-popover-content-transform-origin)] rounded-2xl border bg-popover p-3 text-popover-foreground shadow-xl"
              >
                <div className="flex items-center gap-2">
                  <Signal status={status} level={level} />
                  <SwapText className="text-sm font-extrabold">{heading}</SwapText>
                </div>
                <div className="mt-3 flex items-baseline gap-1.5">
                  <PingText className="text-2xl font-extrabold" />
                  <span className="text-xs text-muted-foreground">{t("dms-calls.calls.connection.ping")}</span>
                </div>
                <div className="mt-1 mb-3">
                  <PingGraph level={level} />
                </div>
                <div className="flex flex-col gap-1.5">
                  <Row label={t("dms-calls.calls.connection.loss")} value={loss} />
                  <Row label={t("dms-calls.calls.connection.jitter")} value={jitter} />
                  <Row label={t("dms-calls.calls.connection.route")} value={route} />
                </div>
                <p className="mt-3 text-[11px] leading-snug text-muted-foreground">
                  {t("dms-calls.calls.connection.about")}
                </p>
              </motion.div>
            </Popover.Content>
          </Popover.Portal>
        )}
      </AnimatePresence>
    </Popover.Root>
  );
}
