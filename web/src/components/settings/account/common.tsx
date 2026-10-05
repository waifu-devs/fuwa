import { CheckIcon, EyeIcon, EyeOffIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useId, type ReactNode } from "react";
import { SPRING } from "@/components/motion";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

export const PASSWORD_MAX = 256;

/** One field of a form, as a flat row under a rule. */
export function Row({ id, label, htmlFor, hint, children }: { id?: string; label: string; htmlFor?: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <div data-setting={id} className="flex flex-col gap-2 border-b border-border/70 py-5 first:pt-0 last:border-b-0">
      <Label htmlFor={htmlFor} className="font-extrabold">
        {label}
      </Label>
      {children}
      {hint && <p className="text-sm text-muted-foreground">{hint}</p>}
    </div>
  );
}

export function Warn({ children }: { children: ReactNode }) {
  return <span className="font-bold text-destructive">{children}</span>;
}

/** A password field with a button that shows what's typed. */
export function PasswordInput({
  id,
  value,
  onChange,
  show,
  onShow,
  autoComplete,
}: {
  id: string;
  value: string;
  onChange: (value: string) => void;
  show: boolean;
  onShow: (show: boolean) => void;
  autoComplete: string;
}) {
  const { t } = useI18n();
  return (
    <div className="relative">
      <Input
        id={id}
        type={show ? "text" : "password"}
        autoComplete={autoComplete}
        maxLength={PASSWORD_MAX}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="h-11 rounded-xl pr-11"
        spellCheck={false}
      />
      <button
        type="button"
        onClick={() => onShow(!show)}
        aria-label={show ? t("accountsettings.shared.hidePasswords") : t("accountsettings.shared.showPasswords")}
        aria-pressed={show}
        className="absolute top-1/2 right-1.5 grid size-8 -translate-y-1/2 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90"
      >
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span key={String(show)} initial={{ opacity: 0, rotate: -40, scale: 0.6 }} animate={{ opacity: 1, rotate: 0, scale: 1 }} exit={{ opacity: 0, rotate: 40, scale: 0.6 }} transition={SPRING}>
            {show ? <EyeOffIcon className="size-4" /> : <EyeIcon className="size-4" />}
          </motion.span>
        </AnimatePresence>
      </button>
    </div>
  );
}

/** A few options side by side in a pill; the highlight glides to the chosen one. */
export function Segmented<T extends string | number>({
  value,
  options,
  onChange,
  label,
  className,
}: {
  value: T;
  options: readonly { value: T; label: string; icon?: ReactNode }[];
  onChange: (value: T) => void;
  label: string;
  className?: string;
}) {
  const group = useId();
  return (
    <div role="radiogroup" aria-label={label} className={cn("inline-flex rounded-xl bg-muted p-1", className)}>
      {options.map((option) => {
        const active = option.value === value;
        return (
          <button
            key={String(option.value)}
            type="button"
            role="radio"
            aria-checked={active}
            onClick={() => onChange(option.value)}
            className={cn(
              "relative flex flex-1 items-center justify-center gap-1.5 rounded-lg px-3 py-1.5 text-sm font-bold whitespace-nowrap transition-colors active:scale-95",
              active ? "text-foreground" : "text-muted-foreground hover:text-foreground",
            )}
          >
            {active && <motion.span layoutId={`segment-${group}`} transition={SPRING} className="absolute inset-0 rounded-lg bg-background shadow-sm" />}
            {option.icon && <span className="relative">{option.icon}</span>}
            <span className="relative">{option.label}</span>
          </button>
        );
      })}
    </div>
  );
}

/** A row's worth of space that shakes, for a form that can't go yet. */
export function useShake() {
  const controls = useAnimationControls();
  return [controls, () => void controls.start({ x: [0, -8, 8, -5, 5, 0], transition: { duration: 0.4 } })] as const;
}

/** Small pill choices; the chosen one fills in with a check. */
export function Chips<T extends string | number>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
  label?: string;
}) {
  return (
    <div role="radiogroup" aria-label={label} className="flex flex-wrap gap-1.5">
      {options.map((o) => {
        const active = o.value === value;
        return (
          <motion.button
            key={String(o.value)}
            type="button"
            role="radio"
            aria-checked={active}
            whileTap={{ scale: 0.92 }}
            onClick={() => onChange(o.value)}
            className={cn(
              "relative rounded-full border px-3 py-1 text-xs font-bold transition-colors",
              active ? "border-primary bg-primary text-primary-foreground" : "text-muted-foreground hover:border-primary/40 hover:text-foreground",
            )}
          >
            <AnimatePresence initial={false}>
              {active && (
                <motion.span initial={{ width: 0, opacity: 0 }} animate={{ width: "auto", opacity: 1 }} exit={{ width: 0, opacity: 0 }} transition={SPRING} className="inline-flex overflow-hidden align-middle">
                  <CheckIcon className="mr-1 size-3" strokeWidth={3} />
                </motion.span>
              )}
            </AnimatePresence>
            {o.label}
          </motion.button>
        );
      })}
    </div>
  );
}
