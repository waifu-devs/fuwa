import { RotateCcwIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useEffect, useId, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";

export const SPRING = { type: "spring", stiffness: 520, damping: 34 } as const;

/**
 * One setting: a title, what it does, and whether it still follows the
 * instance's default. A changed setting can be put back with one click.
 */
export function Setting({
  title,
  hint,
  changed,
  defaultLabel,
  onReset,
  resetting,
  children,
  delay = 0,
}: {
  title: string;
  hint?: ReactNode;
  /** Stored on the instance (overrides the default). */
  changed?: boolean;
  /** The default in words, shown when the setting was changed. */
  defaultLabel?: string;
  onReset?: () => void;
  resetting?: boolean;
  children: ReactNode;
  delay?: number;
}) {
  return (
    <motion.section
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ ...SPRING, delay }}
      className="flex flex-col gap-3 rounded-2xl border bg-background/40 p-4"
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="text-sm font-extrabold">{title}</h3>
          {hint && <p className="mt-0.5 text-xs text-muted-foreground">{hint}</p>}
        </div>
        <AnimatePresence initial={false} mode="popLayout">
          {changed && onReset ? (
            <motion.div
              key="changed"
              initial={{ opacity: 0, scale: 0.8 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.8 }}
              transition={SPRING}
              className="flex shrink-0 items-center gap-1"
            >
              <span className="rounded-full bg-primary/15 px-2 py-0.5 text-[0.65rem] font-bold text-primary uppercase">Changed</span>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={resetting}
                onClick={onReset}
                title={defaultLabel ? `Back to the default: ${defaultLabel}` : "Back to the default"}
                className="group h-7 rounded-full px-2 text-xs"
              >
                <RotateCcwIcon className="size-3.5 transition-transform duration-500 group-hover:-rotate-[360deg]" />
                Reset
              </Button>
            </motion.div>
          ) : (
            <motion.span
              key="default"
              initial={{ opacity: 0, scale: 0.8 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.8 }}
              transition={SPRING}
              className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-[0.65rem] font-bold text-muted-foreground uppercase"
            >
              Default
            </motion.span>
          )}
        </AnimatePresence>
      </div>
      {children}
    </motion.section>
  );
}

export type ChoiceOption<T> = { value: T; label: string; hint: string; icon: ReactNode; disabled?: string };

/** A row of option cards; the selection glides between them. */
export function Choice<T extends string | number>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: ChoiceOption<T>[];
  onChange: (value: T) => void;
}) {
  const group = useId();
  return (
    <div role="radiogroup" className={cn("grid gap-2", options.length === 2 ? "sm:grid-cols-2" : "sm:grid-cols-3")}>
      {options.map((option) => {
        const active = option.value === value;
        return (
          <motion.button
            key={String(option.value)}
            type="button"
            role="radio"
            aria-checked={active}
            disabled={!!option.disabled}
            title={option.disabled}
            whileHover={option.disabled ? undefined : { y: -2 }}
            whileTap={option.disabled ? undefined : { scale: 0.97 }}
            onClick={() => onChange(option.value)}
            className={cn(
              "relative flex flex-col items-start gap-1.5 rounded-xl border p-3 text-left transition-colors",
              active ? "border-primary/60" : "hover:border-primary/30",
              option.disabled && "cursor-not-allowed opacity-50",
            )}
          >
            {active && (
              <motion.span
                layoutId={`choice-${group}`}
                transition={SPRING}
                className="absolute inset-0 -z-0 rounded-xl bg-primary/10 ring-2 ring-primary/50"
              />
            )}
            <span className={cn("relative grid size-8 place-items-center rounded-lg transition-colors", active ? "bg-primary text-primary-foreground" : "bg-muted text-muted-foreground")}>
              <motion.span key={String(active)} initial={active ? { scale: 0.4, rotate: -30 } : false} animate={{ scale: 1, rotate: 0 }} transition={SPRING}>
                {option.icon}
              </motion.span>
            </span>
            <span className="relative text-sm font-bold">{option.label}</span>
            <span className="relative text-xs text-muted-foreground">{option.disabled ?? option.hint}</span>
          </motion.button>
        );
      })}
    </div>
  );
}

/** A labelled on/off switch. */
export function Toggle({
  checked,
  onChange,
  label,
  hint,
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  hint?: string;
  disabled?: boolean;
}) {
  return (
    <label className={cn("flex cursor-pointer items-center justify-between gap-4", disabled && "cursor-not-allowed opacity-60")}>
      <span>
        <span className="block text-sm font-bold">{label}</span>
        {hint && <span className="block text-xs text-muted-foreground">{hint}</span>}
      </span>
      <Switch checked={checked} onCheckedChange={onChange} disabled={disabled} />
    </label>
  );
}

const UNITS = [
  { label: "MB", factor: 1024 ** 2 },
  { label: "GB", factor: 1024 ** 3 },
  { label: "TB", factor: 1024 ** 4 },
];

function splitBytes(bytes: bigint | undefined) {
  if (bytes === undefined) return { amount: "", unit: 1 };
  const n = Number(bytes);
  const unit = [...UNITS].reverse().findIndex((u) => n >= u.factor && Number.isInteger((n / u.factor) * 100));
  const index = unit === -1 ? 0 : UNITS.length - 1 - unit;
  return { amount: String(Math.round((n / UNITS[index]!.factor) * 100) / 100), unit: index };
}

/**
 * A cap that can be off ("no limit") or a number. Sizes take a unit. The
 * number slides in when the cap is switched on.
 */
export function Cap({
  label,
  value,
  onChange,
  bytes = false,
  placeholder,
}: {
  label: string;
  value: bigint | undefined;
  onChange: (value: bigint | undefined) => void;
  bytes?: boolean;
  placeholder?: string;
}) {
  const id = useId();
  const on = value !== undefined;
  const [text, setText] = useState(() => (bytes ? splitBytes(value).amount : (value?.toString() ?? "")));
  const [unit, setUnit] = useState(() => splitBytes(value).unit);

  // Follow outside changes, such as a reset or discarding the draft.
  useEffect(() => {
    const current = bytes ? splitBytes(value) : { amount: value?.toString() ?? "", unit };
    const parsed = parse(text, unit);
    if (parsed !== value) {
      setText(current.amount);
      if (bytes) setUnit(current.unit);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

  function parse(amount: string, unitIndex: number): bigint | undefined {
    const n = Number(amount);
    if (amount.trim() === "" || !Number.isFinite(n) || n < 0) return undefined;
    return BigInt(Math.round(bytes ? n * UNITS[unitIndex]!.factor : n));
  }

  return (
    <div className="flex min-h-9 items-center gap-3">
      <Switch
        id={id}
        checked={on}
        onCheckedChange={(checked) => {
          if (!checked) return onChange(undefined);
          const next = parse(text, unit) ?? (bytes ? BigInt(UNITS[1]!.factor) : 100n);
          if (text.trim() === "") setText(bytes ? "1" : "100");
          if (bytes && text.trim() === "") setUnit(1);
          onChange(next);
        }}
      />
      <label htmlFor={id} className="w-24 shrink-0 text-sm font-bold">
        {label}
      </label>
      <AnimatePresence initial={false} mode="wait">
        {on ? (
          <motion.div
            key="on"
            initial={{ opacity: 0, x: -12 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -12 }}
            transition={SPRING}
            className="flex min-w-0 flex-1 items-center gap-2"
          >
            <Input
              inputMode="decimal"
              value={text}
              aria-label={`${label} limit`}
              onChange={(e) => {
                setText(e.target.value);
                const parsed = parse(e.target.value, unit);
                if (parsed !== undefined) onChange(parsed);
              }}
              className="h-9 w-28 rounded-lg tabular-nums"
            />
            {bytes && (
              <div className="flex rounded-lg bg-muted p-0.5">
                {UNITS.map((u, n) => (
                  <button
                    key={u.label}
                    type="button"
                    onClick={() => {
                      setUnit(n);
                      const parsed = parse(text, n);
                      if (parsed !== undefined) onChange(parsed);
                    }}
                    className={cn("relative rounded-md px-2 py-1 text-xs font-bold transition-colors", unit === n ? "text-foreground" : "text-muted-foreground")}
                  >
                    {unit === n && <motion.span layoutId={`unit-${id}`} transition={SPRING} className="absolute inset-0 rounded-md bg-background shadow-sm" />}
                    <span className="relative">{u.label}</span>
                  </button>
                ))}
              </div>
            )}
          </motion.div>
        ) : (
          <motion.span
            key="off"
            initial={{ opacity: 0, x: 12 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: 12 }}
            transition={SPRING}
            className="text-sm text-muted-foreground"
          >
            {placeholder ?? "No limit"}
          </motion.span>
        )}
      </AnimatePresence>
    </div>
  );
}

/**
 * The bar that slides up while there are unsaved changes. `nudge` changes
 * when someone tries to leave without saving, which shakes it.
 */
export function SaveBar({
  count,
  saving,
  error,
  nudge,
  onSave,
  onDiscard,
  inset = false,
}: {
  count: number;
  saving: boolean;
  error: string | null;
  nudge: number;
  onSave: () => void;
  onDiscard: () => void;
  /** Inside a panel that clips (such as tabs): no bleed, a softer shadow. */
  inset?: boolean;
}) {
  const shake = useAnimationControls();
  useEffect(() => {
    if (nudge) void shake.start({ x: [0, -10, 10, -8, 8, -4, 4, 0], transition: { duration: 0.5 } });
  }, [nudge, shake]);
  return (
    <AnimatePresence>
      {count > 0 && (
        <motion.div
          initial={{ y: 80, opacity: 0 }}
          animate={{ y: 0, opacity: 1 }}
          exit={{ y: 80, opacity: 0 }}
          transition={SPRING}
          className={cn("sticky bottom-0 z-10 mt-4", inset ? "pb-3" : "-mx-2 pb-1")}
        >
          <motion.div
            animate={shake}
            className={cn(
              "flex flex-wrap items-center gap-3 rounded-2xl border bg-popover/95 p-3 pl-4 backdrop-blur transition-colors",
              inset ? "shadow-md" : "shadow-xl",
              nudge && "border-destructive/60",
            )}
          >
            <p className="min-w-0 flex-1 text-sm">
              {error ? (
                <span className="text-destructive first-letter:uppercase">{error}</span>
              ) : (
                <>
                  <span className="font-bold">Unsaved changes</span>{" "}
                  <span className="text-muted-foreground">
                    ({count} {count === 1 ? "setting" : "settings"})
                  </span>
                </>
              )}
            </p>
            <Button type="button" variant="ghost" size="sm" className="rounded-xl" onClick={onDiscard} disabled={saving}>
              Discard
            </Button>
            <Button type="button" size="sm" className="btn rounded-xl px-4 font-bold" onClick={onSave} disabled={saving}>
              {saving ? "Saving…" : "Save changes"}
            </Button>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
