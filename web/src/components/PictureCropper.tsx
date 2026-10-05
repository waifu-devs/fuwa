import { CheckIcon, MoveIcon, RotateCcwIcon, ZoomInIcon, ZoomOutIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent, type WheelEvent } from "react";
import { EASE_OUT, SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Slider } from "@/components/ui/slider";
import { centered, clampCrop, cropped, MAX_ZOOM, PICTURE, placement, zoomAround, type Crop, type PictureKind } from "@/lib/pictures";
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

/**
 * Framing a picture before it's uploaded: drag to move it, scroll, pinch or
 * use the slider to zoom, arrow keys and +/- work too. The frame has the
 * shape people will see (a circle for avatars), and a grid shows while
 * you're moving it.
 */
export function PictureCropper({
  src,
  kind,
  onCancel,
  onDone,
}: {
  /** An object URL for the picture, or null while closed. */
  src: string | null;
  kind: PictureKind;
  onCancel: () => void;
  onDone: (picture: Blob) => void;
}) {
  const { t } = useI18n();
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setImage(null);
    setFailed(null);
    setSaving(false);
    if (!src) return;
    let live = true;
    const img = new Image();
    img.onload = () => live && setImage(img);
    img.onerror = () => live && setFailed(t("workspace.picture.notPicture"));
    img.src = src;
    return () => {
      live = false;
    };
  }, [src, t]);

  return (
    <Dialog open={!!src} onOpenChange={(open) => !open && !saving && onCancel()}>
      <DialogContent wide={kind === "banner"}>
        <DialogHeader title={t(`workspace.picture.frame.${kind}`)} description={t("workspace.picture.frameAbout")} />
        {failed ? (
          <p className="rounded-2xl bg-destructive/10 p-4 text-sm font-bold text-destructive">{failed}</p>
        ) : (
          <Framer
            image={image}
            kind={kind}
            saving={saving}
            onCancel={onCancel}
            onDone={async (crop, frame) => {
              if (!image) return;
              setSaving(true);
              try {
                onDone(await cropped(t, image, crop, kind, frame));
              } catch (err) {
                setFailed(err instanceof Error ? err.message : t("workspace.picture.cropFailed"));
                setSaving(false);
              }
            }}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function Framer({
  image,
  kind,
  saving,
  onCancel,
  onDone,
}: {
  image: HTMLImageElement | null;
  kind: PictureKind;
  saving: boolean;
  onCancel: () => void;
  onDone: (crop: Crop, frame: { width: number; height: number }) => void;
}) {
  const { t, number } = useI18n();
  const shape = PICTURE[kind];
  const box = useRef<HTMLDivElement>(null);
  const [frame, setFrame] = useState({ width: 0, height: 0 });
  const [crop, setCrop] = useState<Crop>({ cx: 0, cy: 0, zoom: 1 });
  const [moving, setMoving] = useState(false);
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinch = useRef<number | null>(null);
  const size = image ? { width: image.naturalWidth, height: image.naturalHeight } : null;

  // The frame fills the dialog's width at the kind's shape.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const measure = () => {
      const width = el.clientWidth;
      setFrame({ width, height: (width * shape.height) / shape.width });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [shape.height, shape.width]);

  useEffect(() => {
    if (size) setCrop(centered(size));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [image]);

  const ready = !!size && frame.width > 0;
  const shown = ready ? clampCrop(crop, size, frame) : crop;
  const at = ready ? placement(shown, size, frame) : { x: 0, y: 0, scale: 1 };
  const update = (next: Crop) => size && setCrop(clampCrop(next, size, frame));
  const zoomTo = (zoom: number) => size && setCrop(zoomAround(shown, zoom / shown.zoom, { x: frame.width / 2, y: frame.height / 2 }, size, frame));

  function local(e: { clientX: number; clientY: number }) {
    const r = box.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  function down(e: PointerEvent) {
    if (!ready || saving) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    pointers.current.set(e.pointerId, local(e));
    pinch.current = null;
    setMoving(true);
  }

  function move(e: PointerEvent) {
    const last = pointers.current.get(e.pointerId);
    if (!last || !size) return;
    const now = local(e);
    pointers.current.set(e.pointerId, now);
    if (pointers.current.size >= 2) {
      const [a, b] = [...pointers.current.values()];
      const distance = Math.hypot(a.x - b.x, a.y - b.y);
      if (pinch.current) setCrop(zoomAround(shown, distance / pinch.current, { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }, size, frame));
      pinch.current = distance;
      return;
    }
    update({ ...shown, cx: shown.cx - (now.x - last.x) / at.scale, cy: shown.cy - (now.y - last.y) / at.scale });
  }

  function up(e: PointerEvent) {
    pointers.current.delete(e.pointerId);
    pinch.current = null;
    if (pointers.current.size === 0) setMoving(false);
  }

  function wheel(e: WheelEvent) {
    if (!size || saving) return;
    setCrop(zoomAround(shown, Math.exp(-e.deltaY * 0.0015), local(e), size, frame));
  }

  function keys(e: KeyboardEvent) {
    if (!size) return;
    const step = 12 / at.scale;
    const moves: Record<string, [number, number]> = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] };
    if (moves[e.key]) {
      e.preventDefault();
      update({ ...shown, cx: shown.cx + moves[e.key][0], cy: shown.cy + moves[e.key][1] });
    } else if (e.key === "+" || e.key === "=") {
      e.preventDefault();
      zoomTo(shown.zoom * 1.1);
    } else if (e.key === "-") {
      e.preventDefault();
      zoomTo(shown.zoom / 1.1);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div
        ref={box}
        tabIndex={0}
        role="application"
        aria-label={t(`workspace.picture.frameAria.${kind}`)}
        onPointerDown={down}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={up}
        onWheel={wheel}
        onKeyDown={keys}
        style={{ height: frame.height || undefined, aspectRatio: frame.height ? undefined : `${shape.width} / ${shape.height}` }}
        className={cn(
          "relative w-full touch-none overflow-hidden rounded-2xl bg-muted outline-none select-none focus-visible:ring-2 focus-visible:ring-primary",
          ready && !saving && (moving ? "cursor-grabbing" : "cursor-grab"),
        )}
      >
        {!ready && <div className="absolute inset-0 animate-pulse bg-muted" />}
        <AnimatePresence>
          {ready && image && (
            <motion.img
              key={image.src}
              src={image.src}
              alt=""
              draggable={false}
              initial={{ opacity: 0, filter: "blur(8px)" }}
              animate={{ opacity: 1, filter: "blur(0px)" }}
              transition={{ duration: 0.45, ease: EASE_OUT }}
              style={{ width: size!.width * at.scale, height: size!.height * at.scale, transform: `translate(${at.x}px, ${at.y}px)` }}
              className="pointer-events-none absolute top-0 left-0 max-w-none origin-top-left"
            />
          )}
        </AnimatePresence>
        {/* Everything outside the shape dims; the shape's edge glows. */}
        <span
          aria-hidden
          className={cn(
            "pointer-events-none absolute shadow-[0_0_0_9999px_rgb(0_0_0/0.5)] ring-2 ring-white/80",
            "inset-0",
            shape.round ? "rounded-full" : kind === "icon" ? "rounded-[32%]" : "rounded-2xl",
          )}
        />
        <AnimatePresence>
          {moving && (
            <motion.span
              aria-hidden
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.2 }}
              className="pointer-events-none absolute inset-0 bg-[linear-gradient(to_right,transparent_33%,rgb(255_255_255/0.35)_33%,rgb(255_255_255/0.35)_calc(33%+1px),transparent_calc(33%+1px),transparent_66%,rgb(255_255_255/0.35)_66%,rgb(255_255_255/0.35)_calc(66%+1px),transparent_calc(66%+1px)),linear-gradient(to_bottom,transparent_33%,rgb(255_255_255/0.35)_33%,rgb(255_255_255/0.35)_calc(33%+1px),transparent_calc(33%+1px),transparent_66%,rgb(255_255_255/0.35)_66%,rgb(255_255_255/0.35)_calc(66%+1px),transparent_calc(66%+1px))]"
            />
          )}
        </AnimatePresence>
        <AnimatePresence>
          {ready && !moving && shown.zoom === 1 && shown.cx === size!.width / 2 && shown.cy === size!.height / 2 && (
            <motion.span
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: 6 }}
              transition={SPRING}
              className="pointer-events-none absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-1.5 rounded-full bg-black/60 px-3 py-1 text-xs font-bold whitespace-nowrap text-white"
            >
              <MoveIcon className="size-3.5" /> {t("workspace.picture.dragToMove")}
            </motion.span>
          )}
        </AnimatePresence>
      </div>

      <div className="flex items-center gap-3">
        <button
          type="button"
          aria-label={t("workspace.picture.zoomOut")}
          disabled={!ready || shown.zoom <= 1}
          onClick={() => zoomTo(shown.zoom / 1.25)}
          className="grid size-9 shrink-0 place-items-center rounded-full text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90 disabled:opacity-40"
        >
          <ZoomOutIcon className="size-4" />
        </button>
        <Slider
          label={t("workspace.picture.zoom")}
          value={shown.zoom}
          min={1}
          max={MAX_ZOOM}
          step={0.01}
          format={(z) => number(Math.round(z * 100) / 100, { style: "percent" })}
          onChange={zoomTo}
          className="flex-1"
        />
        <button
          type="button"
          aria-label={t("workspace.picture.zoomIn")}
          disabled={!ready || shown.zoom >= MAX_ZOOM}
          onClick={() => zoomTo(shown.zoom * 1.25)}
          className="grid size-9 shrink-0 place-items-center rounded-full text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90 disabled:opacity-40"
        >
          <ZoomInIcon className="size-4" />
        </button>
      </div>

      <div className="flex flex-wrap items-center justify-end gap-2">
        <Button
          type="button"
          variant="ghost"
          disabled={!ready || saving}
          onClick={() => size && setCrop(centered(size))}
          className="group mr-auto rounded-xl"
        >
          <RotateCcwIcon className="transition-transform duration-500 group-hover:-rotate-180" /> {t("workspace.picture.reset")}
        </Button>
        <Button type="button" variant="ghost" disabled={saving} onClick={onCancel} className="rounded-xl">
          {t("common.cancel")}
        </Button>
        <Button type="button" disabled={!ready || saving} onClick={() => onDone(shown, frame)} className="btn group rounded-xl px-4 font-bold">
          <CheckIcon className="transition-transform group-hover:scale-125" /> {t("workspace.picture.useIt")}
        </Button>
      </div>
    </div>
  );
}
