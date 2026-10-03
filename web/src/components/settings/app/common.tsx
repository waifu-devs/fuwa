import { CheckIcon, PlusIcon, SparklesIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { MouseEvent, ReactNode } from "react";
import { SPRING } from "@/components/motion";
import { Setting } from "@/components/settings/controls";
import { keycaps } from "@/lib/keybinds";
import { defaultPrefs, setPrefs, usePrefs, type Prefs } from "@/lib/prefs";
import type { Theme } from "@/lib/themes";
import { cn } from "@/lib/utils";

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/**
 * An app setting: like an instance setting, it says whether it still
 * follows the default and can be put back, but it's stored on this device.
 */
export function PrefSetting({
  id,
  title,
  hint,
  keys,
  children,
  delay,
}: {
  id: string;
  title: string;
  hint?: ReactNode;
  /** The settings this row changes, for its Default badge and Reset. */
  keys: (keyof Prefs)[];
  children: ReactNode;
  delay?: number;
}) {
  const prefs = usePrefs((p) => p);
  const defaults = defaultPrefs();
  const changed = keys.some((k) => !same(prefs[k], defaults[k]));
  return (
    <Setting
      id={id}
      title={title}
      hint={hint}
      delay={delay}
      changed={changed}
      onReset={() => setPrefs(Object.fromEntries(keys.map((k) => [k, defaults[k]])) as Partial<Prefs>)}
    >
      {children}
    </Setting>
  );
}

/** Keycaps for a combo; each one pops in, so a recorded shortcut builds up key by key. */
export function Keycaps({ combo, className }: { combo: string; className?: string }) {
  return (
    <span className={cn("inline-flex items-center gap-1", className)}>
      <AnimatePresence initial={false} mode="popLayout">
        {keycaps(combo).map((cap, n) => (
          <motion.kbd
            key={`${n}-${cap}`}
            layout
            initial={{ opacity: 0, scale: 0.4, y: 6 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.4 }}
            transition={{ type: "spring", stiffness: 700, damping: 22 }}
            className="keycap"
          >
            {cap}
          </motion.kbd>
        ))}
      </AnimatePresence>
    </span>
  );
}

/** Theme cards in their own colors; the check glides to the picked one. `onMake` adds a card to make your own. */
export function ThemeGrid({
  themes,
  value,
  onChange,
  onMake,
  id,
}: {
  themes: Theme[];
  value: string;
  onChange: (id: string, e: MouseEvent) => void;
  onMake?: () => void;
  id: string;
}) {
  return (
    <div role="radiogroup" className="grid gap-3 sm:grid-cols-2">
      {themes.map((theme, n) => {
        const t = theme.variant.tokens;
        const active = theme.id === value;
        return (
          <motion.button
            key={theme.id}
            type="button"
            role="radio"
            aria-checked={active}
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: n * 0.04 } }}
            whileHover={{ y: -3 }}
            whileTap={{ scale: 0.97 }}
            onClick={(e) => onChange(theme.id, e)}
            style={{ background: t.background, color: t.foreground, borderColor: active ? t.primary : t.border }}
            className={cn("relative flex flex-col gap-3 overflow-hidden rounded-2xl border-2 p-4 text-left", active && "shadow-lg")}
          >
            <span className="flex items-center gap-2">
              {[t.primary, t.card, t["muted-foreground"], t.border].map((c, i) => (
                <span key={i} className="size-5 rounded-full border" style={{ background: c, borderColor: t.border }} />
              ))}
            </span>
            <span>
              <span className="flex items-center gap-1.5 font-extrabold">
                {theme.name}
                {!theme.builtin && <SparklesIcon className="size-3.5" style={{ color: t.primary }} aria-label="Yours" />}
              </span>
              <span className="block text-xs" style={{ color: t["muted-foreground"] }}>
                {theme.description}
              </span>
            </span>
            {active && (
              <motion.span
                layoutId={`theme-check-${id}`}
                transition={SPRING}
                className="absolute top-3 right-3 grid size-6 place-items-center rounded-full"
                style={{ background: t.primary, color: t["primary-foreground"] }}
              >
                <CheckIcon className="size-4" />
              </motion.span>
            )}
          </motion.button>
        );
      })}
      {onMake && (
        <motion.button
          type="button"
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: themes.length * 0.04 } }}
          whileHover={{ y: -3 }}
          whileTap={{ scale: 0.97 }}
          onClick={onMake}
          className="group flex flex-col items-start justify-between gap-3 rounded-2xl border-2 border-dashed p-4 text-left text-muted-foreground transition-colors hover:border-primary/50 hover:text-primary"
        >
          <span className="grid size-7 place-items-center rounded-full bg-muted transition-colors group-hover:bg-primary group-hover:text-primary-foreground">
            <PlusIcon className="size-4 transition-transform group-hover:rotate-90" />
          </span>
          <span>
            <span className="block font-extrabold">Make your own</span>
            <span className="block text-xs">Colors, a background picture, effects.</span>
          </span>
        </motion.button>
      )}
    </div>
  );
}
