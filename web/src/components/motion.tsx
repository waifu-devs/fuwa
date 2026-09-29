
import { animate, AnimatePresence, motion } from "motion/react";
import { useEffect, useLayoutEffect, useRef, type ReactNode } from "react";
import { cn } from "@/lib/utils";

/** The app's springs and easing, so things move alike everywhere. */
export const SPRING = { type: "spring", stiffness: 520, damping: 34 } as const;
export const SOFT_SPRING = { type: "spring", stiffness: 380, damping: 32 } as const;
export const EASE_OUT = [0.22, 1, 0.36, 1] as const;

const reducedMotion = () =>
  typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Fades and slides its children in the first time they scroll into view. */
export function Reveal({ children, className = "" }: { children: ReactNode; className?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const io = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          el.classList.add("in");
          io.disconnect();
        }
      },
      { threshold: 0.15 },
    );
    io.observe(el);
    return () => io.disconnect();
  }, []);
  return (
    <div ref={ref} className={`reveal ${className}`}>
      {children}
    </div>
  );
}

/** Tilts toward the pointer in 3D and moves a glow under it. */
export function Tilt({ children, className = "", max = 8 }: { children: ReactNode; className?: string; max?: number }) {
  const ref = useRef<HTMLDivElement>(null);

  function onMove(e: React.PointerEvent) {
    const el = ref.current;
    if (!el || reducedMotion() || e.pointerType !== "mouse") return;
    const r = el.getBoundingClientRect();
    const x = (e.clientX - r.left) / r.width;
    const y = (e.clientY - r.top) / r.height;
    el.style.setProperty("--mx", `${x * 100}%`);
    el.style.setProperty("--my", `${y * 100}%`);
    el.style.transform = `perspective(700px) rotateX(${(0.5 - y) * max}deg) rotateY(${(x - 0.5) * max}deg) translateY(-4px)`;
  }

  function onLeave() {
    if (ref.current) ref.current.style.transform = "";
  }

  return (
    <div ref={ref} className={`tilt card-pop ${className}`} onPointerLeave={onLeave} onPointerMove={onMove}>
      {children}
    </div>
  );
}

const GLYPHS = ["♡", "✦", "✧", "★", "❀"];

function spawn(x: number, y: number, opts: { dx: number; dy: number; size: number }) {
  const s = document.createElement("span");
  s.className = "sparkle";
  s.textContent = GLYPHS[Math.floor(Math.random() * GLYPHS.length)];
  s.style.left = `${x}px`;
  s.style.top = `${y}px`;
  s.style.setProperty("--dx", `${opts.dx}px`);
  s.style.setProperty("--dy", `${opts.dy}px`);
  s.style.setProperty("--size", `${opts.size}px`);
  s.style.setProperty("--rot", `${(Math.random() - 0.5) * 360}deg`);
  document.body.appendChild(s);
  s.addEventListener("animationend", () => s.remove());
}

/**
 * Site-wide sparkles: a burst of hearts and stars when pressing any `.btn`
 * or `[data-burst]` element, and a soft trail over `[data-sparkle-zone]`.
 */
export function Sparkles() {
  useEffect(() => {
    if (reducedMotion()) return;
    let last = 0;

    function onDown(e: PointerEvent) {
      const target = (e.target as Element).closest(".btn, [data-burst]");
      if (!target) return;
      for (let i = 0; i < 10; i++) {
        const angle = (Math.PI * 2 * i) / 10 + Math.random() * 0.4;
        const dist = 30 + Math.random() * 40;
        spawn(e.clientX, e.clientY, { dx: Math.cos(angle) * dist, dy: Math.sin(angle) * dist, size: 10 + Math.random() * 10 });
      }
    }

    function onMove(e: PointerEvent) {
      if (e.pointerType !== "mouse") return;
      const now = performance.now();
      if (now - last < 45) return;
      if (!(e.target as Element).closest?.("[data-sparkle-zone]")) return;
      last = now;
      spawn(e.clientX, e.clientY, { dx: (Math.random() - 0.5) * 30, dy: 20 + Math.random() * 30, size: 8 + Math.random() * 8 });
    }

    window.addEventListener("pointerdown", onDown);
    window.addEventListener("pointermove", onMove);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("pointermove", onMove);
    };
  }, []);
  return null;
}

/** Text that slides and fades to its new value when it changes, like a renamed server. */
export function SwapText({ children, className }: { children: string; className?: string }) {
  return (
    <AnimatePresence mode="popLayout" initial={false}>
      <motion.span
        key={children}
        initial={{ opacity: 0, y: "0.6em" }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: "-0.6em" }}
        transition={SPRING}
        className={cn("inline-block max-w-full", className)}
      >
        {children}
      </motion.span>
    </AnimatePresence>
  );
}

/**
 * A small count, like an unread badge, that rolls to its new value: up when
 * it grows, down when it shrinks. Past `max` it reads "max+".
 */
export function Count({ value, max }: { value: number; max?: number }) {
  const text = max !== undefined && value > max ? `${max}+` : value.toLocaleString();
  const previous = useRef(value);
  const up = value >= previous.current;
  useEffect(() => {
    previous.current = value;
  }, [value]);
  return (
    <span className="relative inline-flex overflow-hidden align-bottom tabular-nums">
      <AnimatePresence mode="popLayout" initial={false} custom={up}>
        <motion.span
          key={text}
          custom={up}
          variants={ROLL}
          initial="enter"
          animate="center"
          exit="exit"
          transition={SPRING}
          className="inline-block"
        >
          {text}
        </motion.span>
      </AnimatePresence>
    </span>
  );
}

const ROLL = {
  enter: (up: boolean) => ({ y: up ? "100%" : "-100%", opacity: 0 }),
  center: { y: 0, opacity: 1 },
  exit: (up: boolean) => ({ y: up ? "-100%" : "100%", opacity: 0 }),
};

/**
 * A number that counts up to its value, then glides to each new one.
 * `format` turns it into text, for sizes and the like.
 */
export function CountUp({
  value,
  format = (n) => Math.round(n).toLocaleString(),
  delay = 0,
}: {
  value: number;
  format?: (n: number) => string;
  delay?: number;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const shown = useRef(0);
  const formatRef = useRef(format);
  formatRef.current = format;

  // React never owns the text, so re-renders can't cut the count short.
  useLayoutEffect(() => {
    if (ref.current) ref.current.textContent = formatRef.current(shown.current);
  }, []);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (reducedMotion()) {
      shown.current = value;
      el.textContent = formatRef.current(value);
      return;
    }
    const controls = animate(shown.current, value, {
      duration: 0.9,
      delay,
      ease: EASE_OUT,
      onUpdate: (n) => {
        shown.current = n;
        el.textContent = formatRef.current(n);
      },
    });
    return () => controls.stop();
  }, [value, delay]);

  return <span ref={ref} className="tabular-nums" />;
}
