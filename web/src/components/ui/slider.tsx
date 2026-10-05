import * as SliderPrimitive from "@radix-ui/react-slider";
import { AnimatePresence, m as motion } from "motion/react";
import { useState } from "react";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";

/**
 * One value on a track. The thumb swells while held and a bubble above it
 * shows the value; marks under the track name the stops worth knowing.
 */
export function Slider({
  value,
  onChange,
  onCommit,
  min,
  max,
  step = 1,
  format = String,
  marks = [],
  label,
  className,
}: {
  value: number;
  onChange: (value: number) => void;
  /** Called once when the slider is let go. */
  onCommit?: (value: number) => void;
  min: number;
  max: number;
  step?: number;
  format?: (value: number) => string;
  /** Values to mark under the track, like the default. */
  marks?: { value: number; label: string }[];
  label: string;
  className?: string;
}) {
  const [dragging, setDragging] = useState(false);
  const [keys, setKeys] = useState(false);
  const held = dragging || keys;
  const at = (n: number) => `${((n - min) / (max - min)) * 100}%`;
  return (
    <div className={cn("pt-7 pb-1", className)}>
      <SliderPrimitive.Root
        value={[value]}
        min={min}
        max={max}
        step={step}
        onValueChange={([v]) => v !== undefined && onChange(v)}
        onValueCommit={([v]) => v !== undefined && onCommit?.(v)}
        onPointerDown={() => {
          setDragging(true);
          setKeys(false);
        }}
        onPointerUp={() => setDragging(false)}
        onPointerCancel={() => setDragging(false)}
        onBlur={() => setKeys(false)}
        className="relative flex h-5 w-full touch-none items-center select-none"
      >
        <SliderPrimitive.Track className="relative h-2 grow overflow-hidden rounded-full bg-muted">
          <SliderPrimitive.Range className="absolute h-full rounded-full bg-primary" />
        </SliderPrimitive.Track>
        <SliderPrimitive.Thumb aria-label={label} className="group relative block outline-none" onKeyDown={() => setKeys(true)}>
          <motion.span
            animate={{ scale: held ? 1.25 : 1 }}
            transition={SPRING}
            className="block size-5 rounded-full border-[3px] border-primary bg-background shadow-md group-focus-visible:ring-4 group-focus-visible:ring-ring/40"
          />
          <AnimatePresence>
            {held && (
              <motion.span
                initial={{ opacity: 0, y: 6, scale: 0.6 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, y: 6, scale: 0.6 }}
                transition={SPRING}
                className="absolute bottom-full left-1/2 mb-2 -translate-x-1/2 rounded-lg bg-primary px-2 py-0.5 text-xs font-extrabold whitespace-nowrap text-primary-foreground tabular-nums shadow-md"
              >
                {format(value)}
              </motion.span>
            )}
          </AnimatePresence>
        </SliderPrimitive.Thumb>
      </SliderPrimitive.Root>
      {marks.length > 0 && (
        <div className="relative mt-1.5 h-4 text-[0.7rem] text-muted-foreground">
          {marks.map((m) => (
            <button
              key={m.value}
              type="button"
              onClick={() => {
                onChange(m.value);
                onCommit?.(m.value);
              }}
              style={{ left: at(m.value) }}
              className={cn(
                "absolute -translate-x-1/2 whitespace-nowrap transition-colors hover:text-foreground",
                m.value === value && "font-bold text-primary",
              )}
            >
              {m.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
