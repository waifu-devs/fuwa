import { AlertTriangleIcon, BookOpenIcon, ChevronDownIcon, CpuIcon, Loader2Icon, RotateCcwIcon, SparklesIcon, TurtleIcon, ZapOffIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { SPRING } from "@/components/motion";
import { type I18n, type Key, T, useI18n } from "@/i18n/react";
import { EFFECT_INFO } from "@/lib/backdrop";
import { FALLBACKS, MAX_SHADER_BYTES, SHADER_NAME_MAX, shaderId, shaderProblem, STARTERS, type CustomShader } from "@/lib/effects/custom";
import type { Diagnostic } from "@/lib/effects/gpu";
import { retryShader, useShaderStatus, type ShaderStatus } from "@/lib/effects/status";
import { cn } from "@/lib/utils";

/** Whether this browser can draw effects on the GPU. */
const hasWebGpu = () => typeof navigator !== "undefined" && "gpu" in navigator;

/** How long typing has to pause before the shader is compiled and tried. */
const SETTLE_MS = 450;

/** What each starter is, for its tooltip (its name stays, like any name). */
/**
 * Writing a custom shader: its name, what shows where it can't run, and the
 * WGSL, checked by the GPU's own compiler as you type. Code that compiles goes
 * straight to the preview (and the app behind settings); code that doesn't
 * stays a draft here, with the error at its line, so the backdrop never shows
 * a broken shader.
 */
export function ShaderEditor({ value, onChange }: { value: CustomShader; onChange: (shader: CustomShader) => void }) {
  const { t, number } = useI18n();
  const [draft, setDraft] = useState(value.code);
  // The last draft the compiler looked at, and what it said.
  const [checked, setChecked] = useState<{ code: string; errors: Diagnostic[] }>({ code: value.code, errors: [] });
  const [reference, setReference] = useState(false);
  // A shader set from elsewhere (a starter, an imported theme, Reset) replaces the draft.
  const [seen, setSeen] = useState(value.code);
  if (value.code !== seen) {
    setSeen(value.code);
    setDraft(value.code);
  }
  // The latest shader and handler, for the check that commits a draft (it shouldn't restart when they change).
  const latest = useRef({ value, onChange });
  useLayoutEffect(() => {
    latest.current = { value, onChange };
  });

  // Check what's typed once typing settles; commit it if it's sound. A draft that doesn't compile stays here.
  const dirty = draft !== value.code;
  useEffect(() => {
    if (!dirty) return;
    let live = true;
    const timer = setTimeout(() => {
      const problem = shaderProblem(t, draft);
      const found: Promise<Diagnostic[] | null> = problem
        ? Promise.resolve([{ message: problem, line: null, column: null }])
        : import("@/lib/effects/gpu").then(({ checkShader }) => checkShader(draft)).catch(() => null);
      void found.then((list) => {
        if (!live) return;
        setChecked({ code: draft, errors: list ?? [] });
        if (!list?.length) latest.current.onChange({ ...latest.current.value, code: draft });
      });
    }, SETTLE_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [draft, dirty, t]);
  const checking = dirty && checked.code !== draft;
  const errors = dirty ? checked.errors : [];

  const id = shaderId(value.code);
  const status = useShaderStatus(id);
  const size = useMemo(() => new TextEncoder().encode(draft).length, [draft]);

  return (
    <motion.div
      initial={{ opacity: 0, y: -8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      transition={SPRING}
      className="flex flex-col gap-3 overflow-hidden rounded-2xl border bg-card/60 p-3"
    >
      <div className="flex flex-wrap items-center gap-2">
        <input
          aria-label={t("appsettings.shader.name")}
          value={value.name}
          maxLength={SHADER_NAME_MAX}
          onChange={(e) => onChange({ ...value, name: e.target.value })}
          onBlur={(e) => !e.target.value.trim() && onChange({ ...value, name: t("appsettings.shader.defaultName") })}
          className="min-w-0 flex-1 rounded-lg border-transparent bg-transparent px-1 text-sm font-extrabold outline-none hover:bg-muted/60 focus:bg-muted/60"
        />
        <StatusBadge status={status} checking={checking} broken={errors.length > 0} fallback={value.fallback} onRetry={() => retryShader(id)} />
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        <span className="mr-1 text-[0.65rem] font-extrabold tracking-wide text-muted-foreground uppercase">{t("appsettings.shader.startFrom")}</span>
        {STARTERS.map((starter, n) => (
          <motion.button
            key={starter.id}
            type="button"
            title={t(starter.hint)}
            initial={{ opacity: 0, scale: 0.8 }}
            animate={{ opacity: 1, scale: 1, transition: { ...SPRING, delay: n * 0.04 } }}
            whileHover={{ y: -2 }}
            whileTap={{ scale: 0.92 }}
            onClick={() => onChange({ ...starter.shader, fallback: value.fallback })}
            className={cn(
              "rounded-full border px-2.5 py-1 text-xs font-bold transition-colors hover:border-primary/50 hover:text-primary",
              value.code === starter.shader.code && "border-primary bg-primary/10 text-primary",
            )}
          >
            {starter.name}
          </motion.button>
        ))}
      </div>

      <CodeArea value={draft} onChange={setDraft} errors={errors} />

      <AnimatePresence initial={false}>
        {errors.length > 0 && (
          <motion.ul
            key="errors"
            initial={{ opacity: 0, y: -6 }}
            animate={{ opacity: 1, y: 0, x: [0, -5, 4, -2, 0] }}
            exit={{ opacity: 0, y: -6 }}
            transition={SPRING}
            className="flex flex-col gap-1 text-xs"
          >
            {errors.slice(0, 4).map((error, n) => (
              <li key={n} className="flex gap-2 rounded-lg bg-destructive/10 px-2 py-1.5 text-destructive">
                <AlertTriangleIcon className="mt-0.5 size-3.5 shrink-0" />
                <span>
                  {error.line !== null && <b className="mr-1">{t("appsettings.shader.line", { line: error.line })}</b>}
                  {error.message}
                </span>
              </li>
            ))}
            <li className="px-1 text-muted-foreground">{t("appsettings.shader.keepsLast")}</li>
          </motion.ul>
        )}
      </AnimatePresence>

      <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
        <button type="button" onClick={() => setReference((r) => !r)} className="flex items-center gap-1 font-bold text-primary hover:underline">
          <BookOpenIcon className="size-3.5" />
          {t("appsettings.shader.reference")}
          <motion.span animate={{ rotate: reference ? 180 : 0 }} transition={SPRING}>
            <ChevronDownIcon className="size-3.5" />
          </motion.span>
        </button>
        <span className={cn("tabular-nums text-muted-foreground", size > MAX_SHADER_BYTES && "font-bold text-destructive")}>
          {t("appsettings.shader.size", { size: number(size / 1024, { minimumFractionDigits: 1, maximumFractionDigits: 1 }), max: MAX_SHADER_BYTES / 1024 })}
        </span>
      </div>
      <AnimatePresence initial={false}>{reference && <Reference key="reference" />}</AnimatePresence>

      <div className="flex flex-col gap-1.5">
        <p className="text-[0.65rem] font-extrabold tracking-wide text-muted-foreground uppercase">{t("appsettings.shader.fallback")}</p>
        <div role="radiogroup" className="flex flex-wrap gap-1.5">
          {FALLBACKS.map((fallback) => {
            const active = fallback === value.fallback;
            return (
              <button
                key={fallback}
                type="button"
                role="radio"
                aria-checked={active}
                onClick={() => onChange({ ...value, fallback })}
                className={cn("relative rounded-full px-2.5 py-1 text-xs font-bold transition-colors", active ? "text-primary-foreground" : "text-muted-foreground hover:text-foreground")}
              >
                {active && <motion.span layoutId="fallback-pill" transition={SPRING} className="absolute inset-0 -z-0 rounded-full bg-primary" />}
                <span className="relative">{t(EFFECT_INFO[fallback].name)}</span>
              </button>
            );
          })}
        </div>
      </div>
    </motion.div>
  );
}

type Badge = { key: string; icon: ReactNode; text: string; tone: "ok" | "wait" | "bad"; retry?: boolean };

/** The badge's words for a shader's state. */
function describe(t: I18n["t"], status: ShaderStatus | undefined, checking: boolean, broken: boolean, fallback: CustomShader["fallback"]): Badge {
  const instead = fallback === "none" ? t("appsettings.shader.nothing") : t(EFFECT_INFO[fallback].name);
  if (checking) return { key: "checking", icon: <Loader2Icon className="size-3 animate-spin" />, text: t("appsettings.shader.compiling"), tone: "wait" };
  if (broken) return { key: "broken", icon: <AlertTriangleIcon className="size-3" />, text: t("appsettings.shader.doesNotCompile"), tone: "bad" };
  if (!hasWebGpu()) return { key: "nogpu", icon: <CpuIcon className="size-3" />, text: t("appsettings.shader.noGpu", { fallback: instead }), tone: "wait" };
  if (!status) return { key: "trying", icon: <Loader2Icon className="size-3 animate-spin" />, text: t("appsettings.shader.trying"), tone: "wait" };
  switch (status.state) {
    case "running": {
      const text =
        status.scale >= 0.75 ? t("appsettings.shader.running") : status.scale >= 0.5 ? t("appsettings.shader.runningHalf") : t("appsettings.shader.runningThird");
      return { key: `running-${status.scale}`, icon: <SparklesIcon className="size-3" />, text, tone: "ok" };
    }
    case "slow":
      return { key: "slow", icon: <TurtleIcon className="size-3" />, text: t("appsettings.shader.slow", { fallback: instead }), tone: "bad", retry: true };
    case "stopped":
      return { key: "stopped", icon: <ZapOffIcon className="size-3" />, text: t("appsettings.shader.stopped", { fallback: instead }), tone: "bad", retry: true };
    case "broken":
      return {
        key: "failed",
        icon: <AlertTriangleIcon className="size-3" />,
        text: status.line ? `${t("appsettings.shader.line", { line: status.line })} ${status.message}` : status.message,
        tone: "bad",
        retry: true,
      };
  }
}

/** Where the shader stands: being checked, running (and how sharp), or why its fallback shows. */
function StatusBadge({
  status,
  checking,
  broken,
  fallback,
  onRetry,
}: {
  status: ShaderStatus | undefined;
  checking: boolean;
  broken: boolean;
  fallback: CustomShader["fallback"];
  onRetry: () => void;
}) {
  const { t } = useI18n();
  const badge = describe(t, status, checking, broken, fallback);
  return (
    <div className="flex items-center gap-1.5">
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={badge.key}
          initial={{ opacity: 0, y: 6, scale: 0.9 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: -6, scale: 0.9 }}
          transition={SPRING}
          title={badge.text}
          className={cn(
            "flex max-w-64 items-center gap-1 truncate rounded-full px-2 py-0.5 text-[0.7rem] font-bold",
            badge.tone === "ok" && "bg-primary/15 text-primary",
            badge.tone === "wait" && "bg-muted text-muted-foreground",
            badge.tone === "bad" && "bg-destructive/12 text-destructive",
          )}
        >
          {badge.icon}
          <span className="truncate">{badge.text}</span>
        </motion.span>
      </AnimatePresence>
      {badge.retry && (
        <motion.button
          type="button"
          onClick={onRetry}
          whileHover={{ rotate: -90 }}
          whileTap={{ scale: 0.85 }}
          title={t("appsettings.shader.retry")}
          className="grid size-6 place-items-center rounded-full text-muted-foreground hover:bg-muted hover:text-foreground"
        >
          <RotateCcwIcon className="size-3.5" />
        </motion.button>
      )}
    </div>
  );
}

/** The inputs, as the docs list them (docs/themes.md, Custom shaders). */
const INPUTS: [string, Key][] = [
  ["uv", "appsettings.shader.input.uv"],
  ["fuwa.time", "appsettings.shader.input.time"],
  ["fuwa.res", "appsettings.shader.input.res"],
  ["fuwa.pointer", "appsettings.shader.input.pointer"],
  ["fuwa.primary", "appsettings.shader.input.primary"],
  ["fuwa.accent", "appsettings.shader.input.accent"],
  ["fuwa.background", "appsettings.shader.input.background"],
  ["fuwa.foreground", "appsettings.shader.input.foreground"],
  ["fuwa.intensity", "appsettings.shader.input.intensity"],
  ["hash21(p) noise(p) fbm(p)", "appsettings.shader.input.noise"],
];

function Reference() {
  const { t } = useI18n();
  return (
    <motion.div
      initial={{ opacity: 0, y: -6 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -6 }}
      transition={SPRING}
    >
      <div className="flex flex-col gap-2 rounded-xl bg-muted/50 p-3 text-xs">
        <p>
          <T k="appsettings.shader.write" values={{ signature: <code className="font-mono font-bold text-primary">{"fn shade(uv: vec2f) -> vec4f"}</code> }} />
        </p>
        <dl className="grid gap-x-3 gap-y-1 sm:grid-cols-[auto_1fr]">
          {INPUTS.map(([name, hint]) => (
            <div key={name} className="contents">
              <dt className="font-mono font-bold text-primary">{name}</dt>
              <dd className="text-muted-foreground">{t(hint)}</dd>
            </div>
          ))}
        </dl>
        <p className="text-muted-foreground">
          {t("appsettings.shader.limits")}
        </p>
      </div>
    </motion.div>
  );
}

// ───────────────────────── The code area ─────────────────────────

const KEYWORDS = /^(fn|let|var|const|return|if|else|for|loop|while|break|continue|struct|switch|case|default|true|false)$/;
const TYPES = /^(f32|i32|u32|bool|vec[234][fiu]?|mat[234]x[234]f?|array)$/;
const TOKEN = /(\/\/[^\n]*|\/\*[\s\S]*?(?:\*\/|$))|(\b\d+(?:\.\d*)?(?:e[+-]?\d+)?[fiu]?\b|\.\d+(?:e[+-]?\d+)?f?\b)|([A-Za-z_][A-Za-z0-9_]*)/g;

/** WGSL in colors: comments, numbers, keywords, types and fuwa's own names. */
function highlight(code: string): ReactNode[] {
  const out: ReactNode[] = [];
  let at = 0;
  let n = 0;
  for (const match of code.matchAll(TOKEN)) {
    const [text, comment, number, word] = match;
    if (match.index > at) out.push(code.slice(at, match.index));
    at = match.index + text.length;
    const cls = comment
      ? "text-muted-foreground italic"
      : number
        ? "text-[color:var(--wgsl-number)]"
        : word && KEYWORDS.test(word)
          ? "font-bold text-primary"
          : word && TYPES.test(word)
            ? "text-[color:var(--wgsl-type)]"
            : word === "fuwa" || word === "shade" || word === "uv"
              ? "font-bold text-foreground"
              : null;
    out.push(cls ? <span key={n++} className={cls}>{text}</span> : text);
  }
  if (at < code.length) out.push(code.slice(at));
  // A trailing line end needs something after it to take up its line.
  out.push("\n");
  return out;
}

/** A plain textarea over its highlighted copy, with line numbers and error lines marked. */
function CodeArea({ value, onChange, errors }: { value: string; onChange: (code: string) => void; errors: Diagnostic[] }) {
  const { t } = useI18n();
  const lines = value.split("\n").length;
  const bad = new Set(errors.map((e) => e.line).filter((l): l is number => l !== null));
  const colored = useMemo(() => highlight(value), [value]);

  // Tab indents (two spaces) instead of leaving the editor; Escape still does.
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key !== "Tab" || e.shiftKey || e.metaKey || e.ctrlKey || e.altKey) return;
    e.preventDefault();
    const el = e.currentTarget;
    const { selectionStart: start, selectionEnd: end } = el;
    onChange(`${value.slice(0, start)}  ${value.slice(end)}`);
    requestAnimationFrame(() => el.setSelectionRange(start + 2, start + 2));
  };

  return (
    <div className="shader-code relative max-h-[22rem] overflow-auto rounded-xl border bg-background/80 font-mono text-[0.78rem] leading-5 transition-shadow focus-within:border-primary/60 focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--primary)_18%,transparent)]">
      <div className="flex min-w-max">
        <div aria-hidden className="sticky left-0 z-10 shrink-0 border-r bg-muted/70 py-2 pr-2 pl-3 text-right text-muted-foreground/70 select-none">
          {Array.from({ length: lines }, (_, n) => (
            <div key={n} className={cn("transition-colors", bad.has(n + 1) && "font-bold text-destructive")}>
              {n + 1}
            </div>
          ))}
        </div>
        <div className="relative flex-1">
          {[...bad].map((line) => (
            <motion.div
              key={line}
              aria-hidden
              initial={{ opacity: 0, scaleX: 0.6 }}
              animate={{ opacity: 1, scaleX: 1 }}
              transition={SPRING}
              className="absolute inset-x-0 origin-left bg-destructive/12"
              style={{ top: `calc(0.5rem + ${(line - 1) * 1.25}rem)`, height: "1.25rem" }}
            />
          ))}
          <pre aria-hidden className="pointer-events-none relative m-0 px-3 py-2 whitespace-pre text-foreground/90">
            {colored}
          </pre>
          <textarea
            aria-label={t("appsettings.shader.code")}
            value={value}
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={onKeyDown}
            spellCheck={false}
            autoCapitalize="off"
            autoComplete="off"
            autoCorrect="off"
            wrap="off"
            className="absolute inset-0 size-full resize-none overflow-hidden bg-transparent px-3 py-2 whitespace-pre text-transparent caret-primary outline-none selection:bg-primary/25"
          />
        </div>
      </div>
    </div>
  );
}
