import { AnimatePresence, motion } from "motion/react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { CUSTOM, hasBackdrop, isShader, type Backdrop, type Effect, type ShaderEffect } from "@/lib/backdrop";
import { shaderId, type CustomShader } from "@/lib/effects/custom";
import type { Colors, Painter, Source } from "@/lib/effects/gpu";
import { useShaderStatus } from "@/lib/effects/status";
import { activeBackdrop, activeTheme, reduceMotion, usePrefs } from "@/lib/prefs";
import { shownPicture } from "@/lib/shown";
import type { ThemeTokens } from "@/lib/themes";
import { useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * The picture and effect behind the whole app (lib/backdrop.ts), mounted
 * once. The app's panels turn see-through over it (`data-backdrop` on the
 * root, styles/app.css), as solid as the backdrop's `panels` says.
 *
 * Nothing here re-renders per frame: shader effects draw themselves on their
 * canvas (lib/effects/gpu.ts), and the CSS ones are compositor animations.
 * Both stop while the window is hidden or settings cover the app.
 */
export function AppBackdrop() {
  const backdrop = usePrefs(activeBackdrop);
  const tokens = usePrefs((p) => activeTheme(p).variant.tokens);
  const still = usePrefs(reduceMotion);
  const covered = useUi((u) => u.settings !== null);
  const visible = usePageVisible();
  const on = hasBackdrop(backdrop);

  useEffect(() => {
    const root = document.documentElement;
    if (on) {
      root.dataset.backdrop = "";
      root.style.setProperty("--panels", `${backdrop.panels}%`);
    } else {
      delete root.dataset.backdrop;
      root.style.removeProperty("--panels");
    }
  }, [on, backdrop.panels]);

  if (!on || typeof document === "undefined") return null;
  return createPortal(
    <BackdropLayers backdrop={backdrop} tokens={tokens} running={visible && !covered} still={still} className="app-backdrop" />,
    document.body,
  );
}

/** Whether the page is on screen (not a background tab or a minimized window). */
export function usePageVisible() {
  const [visible, setVisible] = useState(() => typeof document === "undefined" || !document.hidden);
  useEffect(() => {
    const update = () => setVisible(!document.hidden);
    document.addEventListener("visibilitychange", update);
    return () => document.removeEventListener("visibilitychange", update);
  }, []);
  return visible;
}

/** The layers of a backdrop, filling their parent: picture, dimming, effect. Also used for previews. */
export function BackdropLayers({
  backdrop,
  tokens,
  running,
  still,
  className,
}: {
  backdrop: Backdrop;
  tokens: ThemeTokens;
  running: boolean;
  still: boolean;
  className?: string;
}) {
  const picture = shownPicture(backdrop.image);
  return (
    <div aria-hidden className={cn("backdrop-layers", !running && "paused", className)}>
      <AnimatePresence initial={false}>
        {picture && <Picture key={picture} url={picture} backdrop={backdrop} />}
      </AnimatePresence>
      {picture && <div className="backdrop-dim" style={{ opacity: backdrop.dim / 100 }} />}
      <AnimatePresence initial={false}>
        {backdrop.effect !== "none" && (
          <motion.div
            key={backdrop.effect}
            className="backdrop-effect"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.6, ease: "easeOut" }}
          >
            {backdrop.effect === CUSTOM && backdrop.shader ? (
              <CustomLayer shader={backdrop.shader} backdrop={backdrop} tokens={tokens} running={running} still={still} />
            ) : isShader(backdrop.effect) ? (
              <ShaderLayer source={{ effect: backdrop.effect }} css={backdrop.effect} backdrop={backdrop} tokens={tokens} running={running} still={still} />
            ) : (
              <TextureLayer effect={backdrop.effect} intensity={backdrop.intensity} />
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** A picture fades in once it has loaded, so a slow one never pops in half drawn. */
function Picture({ url, backdrop }: { url: string; backdrop: Backdrop }) {
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    const image = new Image();
    image.decoding = "async";
    image.onload = () => setLoaded(true);
    image.src = url;
    return () => {
      image.onload = null;
    };
  }, [url]);
  const tile = backdrop.fit === "tile";
  return (
    <motion.div
      className="backdrop-picture"
      initial={{ opacity: 0, scale: 1.04 }}
      animate={loaded ? { opacity: 1, scale: 1 } : { opacity: 0, scale: 1.04 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.7, ease: [0.22, 1, 0.36, 1] }}
      style={{
        backgroundImage: `url("${url.replace(/["\\]/g, "")}")`,
        backgroundSize: tile ? "auto" : backdrop.fit,
        backgroundRepeat: tile ? "repeat" : "no-repeat",
        filter: backdrop.blur ? `blur(${backdrop.blur}px)` : undefined,
        inset: backdrop.blur ? `-${backdrop.blur * 2}px` : 0,
      }}
    />
  );
}

/** Still textures, drawn by CSS (styles/app.css). */
function TextureLayer({ effect, intensity }: { effect: Effect; intensity: number }) {
  return <div className={cn("fx-texture", `fx-${effect}`)} style={{ opacity: (intensity / 100) * 0.9 }} />;
}

type LayerProps = { backdrop: Backdrop; tokens: ThemeTokens; running: boolean; still: boolean };

/**
 * A custom shader, or its fallback where it can't run: no WebGPU, or the
 * shader is broken, too slow here, or stopped the GPU (lib/effects/status.ts
 * says which; the painter writes it).
 */
function CustomLayer({ shader, ...props }: LayerProps & { shader: CustomShader }) {
  const status = useShaderStatus(shaderId(shader.code));
  const [gpu] = useState(() => typeof navigator !== "undefined" && "gpu" in navigator);
  const fallback = !gpu || (status && status.state !== "running");
  return (
    <AnimatePresence initial={false}>
      <motion.div
        key={fallback ? `fallback-${shader.fallback}` : "custom"}
        className="backdrop-effect"
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        transition={{ duration: 0.5, ease: "easeOut" }}
      >
        {!fallback ? (
          <ShaderLayer source={{ custom: shader.code }} css={shader.fallback} {...props} />
        ) : shader.fallback !== "none" ? (
          <ShaderLayer source={{ effect: shader.fallback }} css={shader.fallback} {...props} />
        ) : null}
      </motion.div>
    </AnimatePresence>
  );
}

/**
 * A shader effect: on the GPU where WebGPU works, else the CSS stand-in for
 * `css` (a custom shader's fallback, for custom ones). The painter is made
 * once per canvas and told about changes; React never renders per frame.
 */
function ShaderLayer({
  source,
  css,
  backdrop,
  tokens,
  running,
  still,
}: LayerProps & {
  source: Source;
  css: ShaderEffect | "none";
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const painter = useRef<Painter | null>(null);
  const [mode, setMode] = useState<"trying" | "gpu" | "css">(() => (typeof navigator !== "undefined" && "gpu" in navigator ? "trying" : "css"));
  const colors = colorsOf(tokens);
  const now = { source, intensity: backdrop.intensity, speed: backdrop.speed, colors, still, running };
  const latest = useRef(now);
  // Kept current for the effects below (layout effects run before them).
  useLayoutEffect(() => {
    latest.current = now;
  });

  // Made once: a painter lives as long as its canvas.
  const tryGpu = useRef(mode === "trying");
  useEffect(() => {
    if (!tryGpu.current || !canvas.current) return;
    let cancelled = false;
    const el = canvas.current;
    // A tick later, so a mount that's undone at once (StrictMode) never claims the canvas.
    void new Promise((resolve) => setTimeout(resolve, 0))
      .then(() => (cancelled ? null : import("@/lib/effects/gpu").then(({ createPainter }) => createPainter(el, () => setMode("css")))))
      .catch(() => null)
      .then((made) => {
        if (cancelled) return made?.dispose();
        if (!made) return setMode("css");
        painter.current = made;
        const { running: run, ...options } = latest.current;
        made.set(options);
        made.setRunning(run);
        setMode("gpu");
      });
    return () => {
      cancelled = true;
      painter.current?.dispose();
      painter.current = null;
    };
  }, []);

  // Fallen back to CSS (the GPU went away): the painter has nothing left to draw on.
  useEffect(() => {
    if (mode !== "css") return;
    painter.current?.dispose();
    painter.current = null;
  }, [mode]);

  const c = colors;
  const key = `${"effect" in source ? source.effect : source.custom}|${backdrop.intensity}|${backdrop.speed}|${still}|${c.c1.join()}|${c.c2.join()}|${c.c3.join()}|${c.c4.join()}`;
  useEffect(() => {
    const { running: _, ...options } = latest.current;
    painter.current?.set(options);
  }, [key]);
  useEffect(() => {
    painter.current?.setRunning(running);
  }, [running, mode]);
  // A resize while still redraws the one frame.
  useEffect(() => {
    if (!still || !canvas.current) return;
    const observer = new ResizeObserver(() => {
      const { running: _, ...options } = latest.current;
      painter.current?.set(options);
    });
    observer.observe(canvas.current);
    return () => observer.disconnect();
  }, [still, mode]);

  if (mode === "css") {
    if (css === "none") return null;
    return (
      <div
        className={cn("fx-css", `fx-${css}`, (still || backdrop.speed === 0) && "still")}
        style={{ opacity: backdrop.intensity / 100, ["--fx-speed" as string]: Math.max(backdrop.speed, 1) / 100 }}
      >
        {css === "petals" && PETALS.map((n) => <span key={n} />)}
      </div>
    );
  }
  return <canvas ref={canvas} className="fx-canvas" />;
}

const PETALS = Array.from({ length: 14 }, (_, n) => n);

// ───────────────────────── Colors ─────────────────────────

const rgb = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);

/** The same hue turned, for a second color that goes with the theme's primary. */
function turn([r, g, b]: number[], degrees: number): number[] {
  const max = Math.max(r!, g!, b!);
  const min = Math.min(r!, g!, b!);
  const l = (max + min) / 2;
  const d = max - min;
  if (d === 0) return [r!, g!, b!];
  const s = d / (1 - Math.abs(2 * l - 1));
  let h = max === r ? ((g! - b!) / d) % 6 : max === g ? (b! - r!) / d + 2 : (r! - g!) / d + 4;
  h = (h * 60 + degrees + 360) % 360;
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  const [r1, g1, b1] = h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x] : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
  return [r1 + m, g1 + m, b1 + m];
}

/** The colors effects draw with: the primary, a neighbour of it, and the page. */
export function colorsOf(tokens: ThemeTokens): Colors {
  const primary = rgb(tokens.primary);
  return { c1: [...primary, 1], c2: [...turn(primary, 48), 1], c3: [...rgb(tokens.background), 1], c4: [...rgb(tokens.foreground), 1] };
}
