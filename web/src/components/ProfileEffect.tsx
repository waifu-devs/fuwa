import { useEffect, useRef, useState, type CSSProperties } from "react";
import {
  builtinEffect,
  IDLE_FADE_MS,
  idleFadeAt,
  introLength,
  planEffect,
  SHAPE_PATHS,
  type Particle,
  type ProfileEffectSpec,
  type Shape,
} from "@/lib/effects/profile";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { reportError, reportTiming } from "@/lib/reports";
import { cn } from "@/lib/utils";

const SVG = "http://www.w3.org/2000/svg";
const LITE_KEY = "fuwa:effects-lite";
/** Frames slower than this (95th percentile) through an intro mean the device struggles: later cards go lite. */
const SLOW_FRAME_MS = 34;
/** Hovering replays the intro, at most this often. */
const REPLAY_GAP_MS = 1200;

/** Set for the rest of the visit once a device dropped frames playing an effect. */
let lite = (() => {
  try {
    return sessionStorage.getItem(LITE_KEY) === "1";
  } catch {
    return false;
  }
})();

function goLite(id: string) {
  if (lite) return;
  lite = true;
  reportError("SlowFrames", `profile-effect/${id}`);
  try {
    sessionStorage.setItem(LITE_KEY, "1");
  } catch {
    // Lite for this page only.
  }
}

/** Times frames for `ms`, then hands over the 95th percentile gap; returns a stop. */
function watchFrames(ms: number, done: (p95: number) => void) {
  const gaps: number[] = [];
  const start = performance.now();
  let last = start;
  let frame = 0;
  const tick = (now: number) => {
    gaps.push(now - last);
    last = now;
    if (now - start < ms) {
      frame = requestAnimationFrame(tick);
      return;
    }
    if (gaps.length < 10) return;
    gaps.sort((a, b) => a - b);
    done(gaps[Math.floor(gaps.length * 0.95)]!);
  };
  frame = requestAnimationFrame(tick);
  return () => cancelAnimationFrame(frame);
}

const templates = new Map<Shape, SVGSVGElement>();

/** A shape's SVG, made once and cloned for each particle. */
function shapeNode(shape: Shape) {
  let svg = templates.get(shape);
  if (!svg) {
    const { d, stroke } = SHAPE_PATHS[shape];
    svg = document.createElementNS(SVG, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("width", "100%");
    svg.setAttribute("height", "100%");
    const path = document.createElementNS(SVG, "path");
    path.setAttribute("d", d);
    if (stroke) {
      path.setAttribute("fill", "none");
      path.setAttribute("stroke", "currentColor");
      path.setAttribute("stroke-width", String(stroke));
      path.setAttribute("stroke-linecap", "round");
    } else {
      path.setAttribute("fill", "currentColor");
    }
    svg.append(path);
    templates.set(shape, svg);
  }
  return svg.cloneNode(true);
}

/** A particle's box: an HTML element, so its transform and opacity animations run on the compositor. */
function particleNode(p: Particle) {
  const el = document.createElement("span");
  const s = el.style;
  s.position = "absolute";
  s.left = "0";
  s.top = "0";
  s.width = s.height = `${p.size}px`;
  s.color = p.color;
  s.willChange = "transform, opacity";
  s.opacity = "0";
  // A glow, or a soft shadow that keeps pale particles readable on light cards. Both are drawn once, not per frame.
  s.filter = p.glow ? `drop-shadow(0 0 ${Math.max(2, Math.round(p.size / 4))}px currentColor)` : "drop-shadow(0 1px 1.5px rgb(0 0 0 / 0.2))";
  el.append(shapeNode(p.shape));
  return el;
}

/**
 * Plays a profile effect over whatever it's put in (a card with `relative`
 * and `overflow-hidden`): the intro once, then the idle loop. It never takes
 * clicks, readers skip it, and it only ever animates transform and opacity.
 *
 * With reduced motion on it shows a still instead, and while it's off screen
 * it pauses. Hovering the card replays the intro. `play` off (picker tiles
 * at rest) shows the still too. An unknown effect id shows nothing.
 *
 * `effect` is a built-in effect's id, or a spec already resolved (an
 * instance's or a server's, from `lib/profile-items.ts`), which plays the
 * same way. Keep a spec's identity stable: a new one starts it over.
 */
export function ProfileEffect({
  effect,
  seed,
  color,
  play = true,
  measure = true,
  replayOnHover = true,
  className,
}: {
  effect: string | ProfileEffectSpec | undefined;
  /** Mixed into the randomness: the person's id, so their effect is theirs. */
  seed: string;
  /** The card's own color, for effects that use it. */
  color?: string;
  play?: boolean;
  /** Watch the intro's frames and report (anonymously) when this device struggles. */
  measure?: boolean;
  replayOnHover?: boolean;
  className?: string;
}) {
  const spec = typeof effect === "string" ? builtinEffect(effect) : effect;
  const calm = usePrefs(reduceMotion);
  const ref = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState<{ width: number; height: number } | null>(null);

  useEffect(() => {
    const root = ref.current;
    if (!root || !spec) return;
    // Rounded, so a card growing by a pixel doesn't restart the effect.
    const read = () => {
      const width = Math.round(root.clientWidth / 8) * 8;
      const height = Math.round(root.clientHeight / 8) * 8;
      setBox((b) => (b && b.width === width && b.height === height ? b : { width, height }));
    };
    read();
    const watch = new ResizeObserver(read);
    watch.observe(root);
    return () => watch.disconnect();
  }, [spec]);

  useEffect(() => {
    const root = ref.current;
    if (!root || !spec || !box || box.width === 0 || box.height === 0) return;
    const moving = play && !calm && typeof root.animate === "function";
    const particles = planEffect(spec, { ...box, seed, lite });
    const intro: Animation[] = [];
    const all: Animation[] = [];
    const nodes = document.createDocumentFragment();
    // The idle loop, already in full swing, fades in under the end of the intro.
    const idle = document.createElement("span");
    idle.style.cssText = "position:absolute;inset:0;will-change:opacity";
    for (const p of particles) {
      if (!moving && !p.still) continue;
      const el = particleNode(p);
      if (!moving) {
        nodes.append(el);
        el.style.transform = p.still!.transform;
        el.style.opacity = String(p.still!.opacity);
        continue;
      }
      (p.phase === "idle" ? idle : nodes).append(el);
      const animation = el.animate(p.keyframes, {
        duration: p.duration,
        delay: p.delay,
        iterationStart: p.start,
        iterations: p.iterations,
        easing: p.easing,
        fill: "backwards",
      });
      all.push(animation);
      if (p.phase === "intro") intro.push(animation);
    }
    if (moving) {
      nodes.append(idle);
      all.push(idle.animate([{ opacity: 0 }, { opacity: 1 }], { duration: IDLE_FADE_MS, delay: idleFadeAt(particles), easing: "ease-out", fill: "both" }));
    }
    root.replaceChildren(nodes);
    if (!moving) return () => root.replaceChildren();

    let started = performance.now();
    const length = introLength(particles);

    // Frame times through the intro, the busiest part.
    const stopWatching =
      measure && !lite && document.visibilityState === "visible"
        ? watchFrames(length, (p95) => {
            reportTiming("profile-effect/intro-frame-p95", p95);
            // Offered effects go in as one name: their ids are the instance's, not the app's.
            if (p95 > SLOW_FRAME_MS) goLite(builtinEffect(spec.id) === spec ? spec.id : "offered");
          })
        : null;

    const card = root.parentElement;
    const replay = () => {
      const now = performance.now();
      if (!replayOnHover || now - started < Math.max(length, REPLAY_GAP_MS)) return;
      started = now;
      for (const a of intro) {
        a.currentTime = 0;
        a.play();
      }
    };
    card?.addEventListener("pointerenter", replay);

    // Off screen (a scrolled-away preview, a tile out of view): nothing runs.
    const seen = new IntersectionObserver(([entry]) => {
      for (const a of all) {
        if (!entry?.isIntersecting && a.playState === "running") a.pause();
        else if (entry?.isIntersecting && a.playState === "paused") a.play();
      }
    });
    seen.observe(root);

    return () => {
      stopWatching?.();
      card?.removeEventListener("pointerenter", replay);
      seen.disconnect();
      for (const a of all) a.cancel();
      root.replaceChildren();
    };
  }, [spec, box, seed, calm, play, measure, replayOnHover]);

  if (!spec) return null;
  return (
    <div
      ref={ref}
      aria-hidden
      data-effect={spec.id}
      style={color ? ({ "--fx-profile": color } as CSSProperties) : undefined}
      className={cn("pointer-events-none absolute inset-0 z-10 overflow-hidden [contain:strict]", className)}
    />
  );
}
