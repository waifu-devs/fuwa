import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";

const LENGTH = 6;

/**
 * Six boxes for a code from an authenticator app. One real input sits under
 * them, so typing, pasting and the phone's one-time-code autofill all work;
 * each digit pops into its box, and the caret glides to the next. Calls
 * `onComplete` when the sixth digit lands. Bump `shake` to shake it (a wrong
 * code), which also clears it.
 */
export function CodeInput({
  onComplete,
  disabled,
  shake = 0,
  autoFocus = true,
  id,
  label = "Code",
}: {
  onComplete: (code: string) => void;
  disabled?: boolean;
  shake?: number;
  autoFocus?: boolean;
  id?: string;
  label?: string;
}) {
  const [value, setValue] = useState("");
  const [focused, setFocused] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const controls = useAnimationControls();

  useEffect(() => {
    if (!shake) return;
    void controls.start({ x: [0, -10, 10, -6, 6, 0], transition: { duration: 0.4 } });
    setValue("");
    input.current?.focus();
  }, [shake, controls]);

  function change(next: string) {
    const digits = next.replace(/\D/g, "").slice(0, LENGTH);
    setValue(digits);
    if (digits.length === LENGTH) onComplete(digits);
  }

  const caret = Math.min(value.length, LENGTH - 1);
  return (
    <motion.div animate={controls} className="relative w-fit" onClick={() => input.current?.focus()}>
      <input
        ref={input}
        id={id}
        aria-label={label}
        value={value}
        onChange={(e) => change(e.target.value)}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        inputMode="numeric"
        autoComplete="one-time-code"
        autoFocus={autoFocus}
        disabled={disabled}
        maxLength={LENGTH}
        className="absolute inset-0 z-10 w-full cursor-text opacity-0"
      />
      <div className="flex gap-2" aria-hidden>
        {Array.from({ length: LENGTH }, (_, n) => {
          const digit = value[n];
          const here = focused && n === caret && !disabled;
          return (
            <div
              key={n}
              className={cn(
                "relative grid h-14 w-11 place-items-center rounded-xl border-2 bg-background text-2xl font-extrabold tabular-nums transition-colors sm:w-12",
                n === 3 && "ml-2",
                digit ? "border-primary/50" : "border-border",
                disabled && "opacity-60",
              )}
            >
              {here && (
                <motion.span
                  layoutId={`code-caret-${id ?? "code"}`}
                  className="absolute -inset-[2px] rounded-xl border-2 border-primary shadow-[0_0_0_4px_color-mix(in_srgb,var(--primary)_20%,transparent)]"
                  transition={{ type: "spring", stiffness: 600, damping: 38 }}
                />
              )}
              <AnimatePresence mode="popLayout" initial={false}>
                {digit ? (
                  <motion.span
                    key={`${n}-${digit}`}
                    initial={{ scale: 0.3, y: 8, opacity: 0 }}
                    animate={{ scale: 1, y: 0, opacity: 1 }}
                    exit={{ scale: 0.5, opacity: 0 }}
                    transition={{ type: "spring", stiffness: 700, damping: 22 }}
                  >
                    {digit}
                  </motion.span>
                ) : here ? (
                  <motion.span
                    key="blink"
                    className="h-6 w-0.5 rounded-full bg-primary"
                    animate={{ opacity: [1, 0, 1] }}
                    transition={{ duration: 1, repeat: Infinity, ease: "linear" }}
                  />
                ) : null}
              </AnimatePresence>
            </div>
          );
        })}
      </div>
    </motion.div>
  );
}
