import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { usePageVisible } from "@/hooks/use-page-visible";
import type { Colors, Painter } from "@/lib/effects/gpu";
import type { EffectShader as Shader } from "@/lib/effects/profile";
import { shownTheme, usePrefs } from "@/lib/prefs";
import type { ThemeTokens } from "@/lib/themes";

/**
 * A profile effect's shader (docs/profile-effects.md), drawn across the card
 * under its particles by the same painter as custom backdrop shaders
 * (`lib/effects/gpu.ts`, loaded only here and there): checked, compiled and
 * timed before it draws, at most 30 frames a second, and never while the
 * card is off screen or the page hidden. A shader that can't run here (no
 * WebGPU, a problem in it, too slow, it stopped the GPU) draws nothing, and
 * the particles play as ever. `still` draws one frame: reduced motion, or a
 * picker tile at rest.
 *
 * Its inputs are a card's: `fuwa.primary` is the theme's primary,
 * `fuwa.accent` the card's own color, `fuwa.background` the card and
 * `fuwa.foreground` its text.
 */
export function EffectShader({ shader, color, still }: { shader: Shader; color?: string; still: boolean }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const painter = useRef<Painter | null>(null);
  // Lost: no GPU here, or it went away. Out: this shader can't run (it stays laid out, for the next one).
  const [lost, setLost] = useState(false);
  const [out, setOut] = useState(false);
  const [seen, setSeen] = useState(false);
  const visible = usePageVisible();
  const tokens = usePrefs((p) => shownTheme(p).variant.tokens);
  const colors = cardColors(tokens, color);
  const options = { source: { custom: shader.code }, intensity: shader.intensity * 100, speed: shader.speed * 100, colors, still };
  const latest = useRef(options);
  useLayoutEffect(() => {
    latest.current = options;
  });
  const running = seen && visible;

  // Only while the card is on screen.
  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    const watch = new IntersectionObserver(([entry]) => setSeen(!!entry?.isIntersecting));
    watch.observe(el);
    return () => watch.disconnect();
  }, []);

  // Made once: a painter lives as long as its canvas.
  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    let cancelled = false;
    // A tick later, so a mount that's undone at once (StrictMode) never claims the canvas.
    void new Promise((resolve) => setTimeout(resolve, 0))
      .then(() => (cancelled ? null : import("@/lib/effects/gpu").then(({ createPainter }) => createPainter(el, () => setLost(true), () => setOut(true)))))
      .catch(() => null)
      .then((made) => {
        if (cancelled) return made?.dispose();
        if (!made) return setLost(true);
        painter.current = made;
        made.set(latest.current);
      });
    return () => {
      cancelled = true;
      painter.current?.dispose();
      painter.current = null;
    };
  }, []);

  const c = colors;
  const key = `${shader.code}|${shader.intensity}|${shader.speed}|${still}|${c.c1.join()}|${c.c2.join()}|${c.c3.join()}|${c.c4.join()}`;
  useEffect(() => {
    // A new shader (an edit in settings) gets its chance, even after the last one couldn't run.
    setOut(false);
    painter.current?.set(latest.current);
  }, [key]);
  useEffect(() => {
    painter.current?.setRunning(running);
  }, [running]);

  if (lost) return null;
  return <canvas ref={canvas} aria-hidden hidden={out} className="pointer-events-none absolute inset-0 z-10 size-full" />;
}

// ───────────────────────── Colors ─────────────────────────

const hexRgb = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);

/** A card's color as the effects give it (`#rrggbb`, or `hsl(h s% l%)` picked from an id), as 0..1 RGB; null for anything else. */
export function cssRgb(css: string | undefined): number[] | null {
  if (!css) return null;
  if (/^#[0-9a-f]{6}$/i.test(css)) return hexRgb(css);
  const hsl = /^hsl\(\s*([\d.]+)(?:deg)?[\s,]+([\d.]+)%[\s,]+([\d.]+)%\s*\)$/i.exec(css);
  if (!hsl) return null;
  const h = Number(hsl[1]) % 360;
  const s = Number(hsl[2]) / 100;
  const l = Number(hsl[3]) / 100;
  const k = (n: number) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  return [0, 8, 4].map((n) => l - a * Math.max(-1, Math.min(k(n) - 3, 9 - k(n), 1)));
}

/** The colors a card's shader draws with: the theme's primary, the card's own color, the card and its text. */
function cardColors(tokens: ThemeTokens, color: string | undefined): Colors {
  const primary = hexRgb(tokens.primary);
  return {
    c1: [...primary, 1],
    c2: [...(cssRgb(color) ?? primary), 1],
    c3: [...hexRgb(tokens.card), 1],
    c4: [...hexRgb(tokens["card-foreground"]), 1],
  };
}
