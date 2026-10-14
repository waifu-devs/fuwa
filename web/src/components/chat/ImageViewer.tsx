import * as DialogPrimitive from "@radix-ui/react-dialog";
import { ChevronLeftIcon, ChevronRightIcon, DownloadIcon, ExternalLinkIcon, XIcon, ZoomInIcon, ZoomOutIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { useI18n } from "@/i18n/react";
import { shortName } from "@/lib/attachments";
import { formatBytes } from "@/lib/format";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { cn } from "@/lib/utils";
import { actualScale, FITTED, fitted, panBy, STEP, stepIndex, toggleAt, wheelFactor, zoomAt, type Size, type View } from "@/lib/zoom";

/** One picture the viewer can show. */
export type ViewerPicture = {
  /** Tells it apart from the message's other pictures. */
  key: string;
  /** What's drawn: a fuwa instance's address, or a blob this device opened. */
  src: string;
  name: string;
  bytes?: number;
  /** Its size as the sender's app gave it, used until it loads. */
  width?: number;
  height?: number;
  /** Where it's opened in a new tab; none for one only this device could open. */
  original?: string;
  save?: () => void;
};

/** Room kept clear around the picture, and under it for the caption. */
const EDGE = 24;
const CAPTION = 64;

const roomOf = (el: HTMLElement | null): Size =>
  el
    ? { width: Math.max(1, el.clientWidth - EDGE * 2), height: Math.max(1, el.clientHeight - EDGE * 2) }
    : { width: Math.max(1, (typeof window === "undefined" ? 800 : window.innerWidth) - EDGE * 2), height: Math.max(1, (typeof window === "undefined" ? 600 : window.innerHeight) - CAPTION - EDGE * 2) };

/**
 * Saves a picture under its name. Where the browser won't name a download
 * from another site, it's fetched first; failing that it opens in a tab.
 * The link is never put on screen (streamer mode keeps the address hidden).
 */
export function savePicture(url: string, name: string) {
  const click = (href: string) => {
    const a = document.createElement("a");
    a.href = href;
    a.download = name;
    a.rel = "noopener";
    document.body.append(a);
    a.click();
    a.remove();
  };
  let same = false;
  try {
    same = new URL(url, location.href).origin === location.origin;
  } catch {
    return;
  }
  if (same) return click(url);
  fetch(url, { mode: "cors", credentials: "omit" })
    .then((r) => (r.ok ? r.blob() : Promise.reject(new Error(String(r.status)))))
    .then((blob) => {
      const href = URL.createObjectURL(blob);
      click(href);
      setTimeout(() => URL.revokeObjectURL(href), 60_000);
    })
    .catch(() => openOriginal(url));
}

/** Opens a picture in a new tab, telling that tab nothing about this one. */
export function openOriginal(url: string) {
  window.open(url, "_blank", "noopener,noreferrer");
}

/**
 * Pictures full size over everything. The wheel, a trackpad pinch or two
 * fingers zoom around where they point; a zoomed picture is dragged to look
 * around; a double click goes between fitted and 1:1. The arrows (on screen
 * and on the keyboard) step through the message's other pictures, + - and 0
 * zoom, and Escape, a click outside or the button close it. Each picture
 * opens fitted. `index` is the one shown, `null` when it's closed.
 */
export function ImageViewer({
  pictures,
  index,
  onIndex,
  onClose,
}: {
  pictures: ViewerPicture[];
  index: number | null;
  onIndex: (index: number) => void;
  onClose: () => void;
}) {
  const picture = index === null ? undefined : pictures[Math.min(index, pictures.length - 1)];
  return (
    <DialogPrimitive.Root open={!!picture} onOpenChange={(open) => !open && onClose()}>
      <AnimatePresence>
        {picture && index !== null && (
          <DialogPrimitive.Portal forceMount>
            <DialogPrimitive.Overlay asChild forceMount>
              <motion.div
                className="fixed inset-0 z-50 bg-black/85 backdrop-blur-sm"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.18 }}
              />
            </DialogPrimitive.Overlay>
            <Stage pictures={pictures} index={Math.min(index, pictures.length - 1)} picture={picture} onIndex={onIndex} onClose={onClose} />
          </DialogPrimitive.Portal>
        )}
      </AnimatePresence>
    </DialogPrimitive.Root>
  );
}

function Stage({
  pictures,
  index,
  picture,
  onIndex,
  onClose,
}: {
  pictures: ViewerPicture[];
  index: number;
  picture: ViewerPicture;
  onIndex: (index: number) => void;
  onClose: () => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const calm = usePrefs(reduceMotion);
  const stage = useRef<HTMLDivElement>(null);
  const [room, setRoom] = useState<Size>(() => roomOf(null));
  // Each picture's own zoom and size, kept with which picture they're for, so stepping to another starts it fitted.
  const [zoom, setZoom] = useState<{ for: string; view: View; smooth: boolean }>({ for: picture.key, view: FITTED, smooth: false });
  const [loaded, setLoaded] = useState<{ for: string; size: Size } | null>(null);
  const [dragging, setDragging] = useState(false);
  const view = zoom.for === picture.key ? zoom.view : FITTED;
  const natural: Size =
    loaded?.for === picture.key
      ? loaded.size
      : picture.width && picture.height
        ? { width: picture.width, height: picture.height }
        : { width: room.width, height: room.height };
  const size = fitted(natural, room);
  const many = pictures.length > 1;

  // What the handlers below read, as it is right now.
  const now = useRef({ key: picture.key, natural, room });
  now.current = { key: picture.key, natural, room };
  const change = (next: (view: View) => View, smooth: boolean) =>
    setZoom((z) => {
      const { key } = now.current;
      return { for: key, view: next(z.for === key ? z.view : FITTED), smooth };
    });

  useEffect(() => {
    const el = stage.current;
    if (!el) return;
    const resize = new ResizeObserver(() => setRoom(roomOf(el)));
    resize.observe(el);
    // Wheels zoom here and never scroll the chat behind; React's wheel handlers can't stop that.
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      const box = el.getBoundingClientRect();
      const at = { x: event.clientX - box.left - box.width / 2, y: event.clientY - box.top - box.height / 2 };
      const delta = event.deltaMode === 1 ? event.deltaY * 33 : event.deltaY;
      const factor = wheelFactor(delta, event.ctrlKey);
      change((v) => zoomAt(v, v.scale * factor, at, now.current.natural, now.current.room), false);
    };
    el.addEventListener("wheel", wheel, { passive: false });
    return () => {
      resize.disconnect();
      el.removeEventListener("wheel", wheel);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Fingers and the mouse on the picture: one drags a zoomed picture, two pinch.
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const gesture = useRef({ moved: false, travel: 0 });
  const middleOf = (el: HTMLElement, x: number, y: number) => {
    const box = el.getBoundingClientRect();
    return { x: x - box.left - box.width / 2, y: y - box.top - box.height / 2 };
  };
  const down = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 && event.pointerType === "mouse") return;
    pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (pointers.current.size === 1) gesture.current = { moved: false, travel: 0 };
    if (pointers.current.size === 2) {
      for (const id of pointers.current.keys()) event.currentTarget.setPointerCapture?.(id);
      gesture.current.moved = true;
    }
  };
  const move = (event: PointerEvent<HTMLDivElement>) => {
    const last = pointers.current.get(event.pointerId);
    if (!last) return;
    const el = event.currentTarget;
    const before = [...pointers.current.values()];
    pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (before.length >= 2) {
      const after = [...pointers.current.values()];
      const [a0, b0] = before;
      const [a1, b1] = after;
      const was = Math.hypot(a0.x - b0.x, a0.y - b0.y) || 1;
      const is = Math.hypot(a1.x - b1.x, a1.y - b1.y) || 1;
      const mid0 = middleOf(el, (a0.x + b0.x) / 2, (a0.y + b0.y) / 2);
      const mid1 = middleOf(el, (a1.x + b1.x) / 2, (a1.y + b1.y) / 2);
      const { natural, room } = now.current;
      change((v) => panBy(zoomAt(v, (v.scale * is) / was, mid0, natural, room), mid1.x - mid0.x, mid1.y - mid0.y, natural, room), false);
      return;
    }
    const dx = event.clientX - last.x;
    const dy = event.clientY - last.y;
    gesture.current.travel += Math.abs(dx) + Math.abs(dy);
    if (view.scale <= 1) return;
    if (!gesture.current.moved && gesture.current.travel > 4) {
      gesture.current.moved = true;
      el.setPointerCapture?.(event.pointerId);
      setDragging(true);
    }
    if (gesture.current.moved) change((v) => panBy(v, dx, dy, now.current.natural, now.current.room), false);
  };
  const up = (event: PointerEvent<HTMLDivElement>) => {
    pointers.current.delete(event.pointerId);
    if (pointers.current.size === 0) setDragging(false);
  };

  const zoomBy = (factor: number) => change((v) => zoomAt(v, v.scale * factor, { x: 0, y: 0 }, now.current.natural, now.current.room), true);
  const step = (by: 1 | -1) => many && onIndex(stepIndex(index, pictures.length, by));
  const keys = (event: KeyboardEvent) => {
    if (event.altKey || event.metaKey || (event.ctrlKey && event.key !== "0")) return;
    const handled = (() => {
      switch (event.key) {
        case "ArrowLeft":
          return step(-1), many;
        case "ArrowRight":
          return step(1), many;
        case "+":
        case "=":
          return zoomBy(STEP), true;
        case "-":
        case "_":
          return zoomBy(1 / STEP), true;
        case "0":
          return change(() => FITTED, true), true;
        default:
          return false;
      }
    })();
    if (handled) event.preventDefault();
  };

  const actual = actualScale(natural, room);
  const percent = Math.round((view.scale / actual) * 100);
  const zoomed = view.scale > 1.001;
  const smooth = zoom.smooth && !calm;

  return (
    <DialogPrimitive.Content asChild forceMount aria-describedby={undefined} onKeyDown={keys}>
      <motion.div className="fixed inset-0 z-50 flex flex-col outline-none" exit={{ opacity: 0 }} transition={{ duration: 0.15 }}>
        <DialogPrimitive.Title className="sr-only">{picture.name}</DialogPrimitive.Title>
        <div
          ref={stage}
          onPointerDown={down}
          onPointerMove={move}
          onPointerUp={up}
          onPointerCancel={up}
          // A click outside the picture closes it; the end of a drag or a pinch doesn't.
          onClick={(event) => {
            if (gesture.current.moved || event.target !== event.currentTarget) return;
            onClose();
          }}
          className="relative min-h-0 flex-1 touch-none overflow-hidden select-none"
        >
          <div
            style={{
              width: size.width,
              height: size.height,
              transform: `translate(-50%, -50%) translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
              transition: smooth ? "transform 200ms cubic-bezier(0.2, 0.8, 0.2, 1)" : undefined,
            }}
            onDoubleClick={(event) => {
              const el = stage.current;
              if (!el) return;
              const at = middleOf(el, event.clientX, event.clientY);
              change((v) => toggleAt(v, at, now.current.natural, now.current.room), true);
            }}
            className={cn("absolute top-1/2 left-1/2 will-change-transform", zoomed ? (dragging ? "cursor-grabbing" : "cursor-grab") : "cursor-zoom-in")}
          >
            <motion.img
              key={picture.key}
              src={picture.src}
              alt={picture.name}
              draggable={false}
              decoding="async"
              onLoad={(event) => {
                const img = event.currentTarget;
                if (img.naturalWidth > 0) setLoaded({ for: picture.key, size: { width: img.naturalWidth, height: img.naturalHeight } });
              }}
              initial={{ opacity: 0, scale: 0.94 }}
              animate={{ opacity: 1, scale: 1 }}
              transition={{ type: "spring", stiffness: 420, damping: 32 }}
              className="pointer-events-none size-full rounded-lg object-contain shadow-2xl"
            />
          </div>
          {many && (
            <>
              <Side label={t("chat.viewer.previous")} onClick={() => step(-1)} className="left-3">
                <ChevronLeftIcon className="size-6" />
              </Side>
              <Side label={t("chat.viewer.next")} onClick={() => step(1)} className="right-3">
                <ChevronRightIcon className="size-6" />
              </Side>
            </>
          )}
        </div>
        <div className="flex shrink-0 justify-center px-3 pb-4" style={{ minHeight: CAPTION }}>
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ type: "spring", stiffness: 420, damping: 32, delay: 0.05 }}
            className="flex max-w-full items-center gap-1 self-end rounded-full bg-black/60 py-1.5 pr-1.5 pl-4 text-sm text-white"
          >
            <span className="min-w-0 truncate font-bold" title={picture.name}>
              {shortName(picture.name, 48)}
            </span>
            {!!picture.bytes && <span className="ml-1 shrink-0 text-white/70 tabular-nums">{formatBytes(lang, picture.bytes)}</span>}
            {many && <span className="ml-1 shrink-0 text-white/70 tabular-nums">{t("chat.viewer.counter", { n: index + 1, total: pictures.length })}</span>}
            <span aria-hidden className="mx-1.5 h-5 w-px shrink-0 bg-white/20" />
            <Round label={t("chat.viewer.zoomOut")} onClick={() => zoomBy(1 / STEP)} disabled={!zoomed}>
              <ZoomOutIcon className="size-4" />
            </Round>
            <button
              type="button"
              onClick={() => change((v) => (zoomed ? FITTED : toggleAt(v, { x: 0, y: 0 }, now.current.natural, now.current.room)), true)}
              title={zoomed ? t("chat.viewer.fit") : t("chat.viewer.actual")}
              className="h-8 w-14 shrink-0 rounded-full text-xs font-bold tabular-nums transition-colors hover:bg-white/15"
            >
              {percent}%
            </button>
            <Round label={t("chat.viewer.zoomIn")} onClick={() => zoomBy(STEP)}>
              <ZoomInIcon className="size-4" />
            </Round>
            {picture.original && (
              <Round label={t("chat.viewer.original")} onClick={() => openOriginal(picture.original!)}>
                <ExternalLinkIcon className="size-4" />
              </Round>
            )}
            {picture.save && (
              <Round label={t("chat.attachments.download", { name: picture.name })} onClick={picture.save}>
                <DownloadIcon className="size-4" />
              </Round>
            )}
            <DialogPrimitive.Close className="grid size-8 shrink-0 place-items-center rounded-full transition hover:rotate-90 hover:bg-white/15">
              <XIcon className="size-4" />
              <span className="sr-only">{t("common.close")}</span>
            </DialogPrimitive.Close>
          </motion.div>
        </div>
      </motion.div>
    </DialogPrimitive.Content>
  );
}

function Round({ label, onClick, disabled, children }: { label: string; onClick: () => void; disabled?: boolean; children: ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      title={label}
      className="grid size-8 shrink-0 place-items-center rounded-full transition-colors hover:bg-white/15 disabled:opacity-40 disabled:hover:bg-transparent"
    >
      {children}
    </button>
  );
}

/** The previous and next buttons, at the sides. */
function Side({ label, onClick, className, children }: { label: string; onClick: () => void; className: string; children: ReactNode }) {
  return (
    <motion.button
      type="button"
      onClick={onClick}
      // Never the start of a drag or a click outside.
      onPointerDown={(event) => event.stopPropagation()}
      aria-label={label}
      title={label}
      whileHover={{ scale: 1.08 }}
      whileTap={{ scale: 0.92 }}
      className={cn("absolute top-1/2 z-10 grid size-11 -translate-y-1/2 place-items-center rounded-full bg-black/55 text-white shadow-lg transition-colors hover:bg-black/75", className)}
    >
      {children}
    </motion.button>
  );
}
