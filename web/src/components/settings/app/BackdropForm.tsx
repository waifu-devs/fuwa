import { CheckIcon, CodeXmlIcon, Grid2x2Icon, ImageOffIcon, ImagePlusIcon, Loader2Icon, Maximize2Icon, Minimize2Icon, Trash2Icon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState, type DragEvent, type ReactNode } from "react";
import { MediaPurpose } from "@/gen/fuwa/v1/media_pb";
import { deleteBackground, keepBackground, listBackgrounds, run, uploadPicture } from "@/fuwa/actions";
import { SPRING } from "@/components/motion";
import { Choice } from "@/components/settings/controls";
import { Slider } from "@/components/ui/slider";
import { ShaderEditor } from "@/components/settings/app/ShaderEditor";
import { CUSTOM, EFFECT_INFO, EFFECTS, isShader, LIMITS, moves, type Backdrop, type Effect } from "@/lib/backdrop";
import { DEFAULT_SHADER } from "@/lib/effects/custom";
import { backgroundPicture, PICTURE_TYPES } from "@/lib/pictures";
import { shownPicture } from "@/lib/shown";
import { cn } from "@/lib/utils";

/** Whether this browser can draw effects on the GPU. */
export const hasWebGpu = () => typeof navigator !== "undefined" && "gpu" in navigator;

/**
 * Everything about a backdrop: the picture (from your backgrounds on this
 * instance), how it sits, and the effect over it. Used for the app's
 * backdrop and for a custom theme's own.
 */
export function BackdropForm({
  instanceKey,
  value,
  onChange,
}: {
  instanceKey?: string;
  value: Backdrop;
  onChange: (patch: Partial<Backdrop>) => void;
}) {
  return (
    <div className="flex flex-col gap-6">
      <Block title="Picture" hint="Uploaded to your instance and kept with your account there. Only fuwa instances serve backgrounds.">
        <PictureLibrary instanceKey={instanceKey} value={value.image} onChange={(image) => onChange({ image })} />
        <AnimatePresence initial={false}>
          {value.image && (
            <motion.div
              initial={{ opacity: 0, y: -6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={SPRING}
              className="flex flex-col gap-2 overflow-hidden"
            >
              <Choice
                value={value.fit}
                onChange={(fit) => onChange({ fit })}
                options={[
                  { value: "cover", label: "Fill", hint: "Covers the window.", icon: <Maximize2Icon className="size-4" /> },
                  { value: "contain", label: "Fit", hint: "The whole picture shows.", icon: <Minimize2Icon className="size-4" /> },
                  { value: "tile", label: "Tile", hint: "Repeats, for patterns.", icon: <Grid2x2Icon className="size-4" /> },
                ]}
              />
              <Labeled label="Dim" shown={`${value.dim}%`}><Slider label="Dim" value={value.dim} min={LIMITS.dim[0]} max={LIMITS.dim[1]} format={(n) => `${n}%`} onChange={(dim) => onChange({ dim })} className="pt-6" /></Labeled>
              <Labeled label="Blur" shown={`${value.blur}px`}><Slider label="Blur" value={value.blur} min={LIMITS.blur[0]} max={LIMITS.blur[1]} format={(n) => `${n}px`} onChange={(blur) => onChange({ blur })} className="pt-6" /></Labeled>
            </motion.div>
          )}
        </AnimatePresence>
      </Block>

      <Block
        title="Effect"
        hint={
          hasWebGpu()
            ? "Moving effects are drawn on your GPU, at most 30 frames a second, and stop while fuwa is hidden."
            : "This browser has no WebGPU, so moving effects use a lighter version drawn with CSS."
        }
      >
        <EffectGrid
          value={value.effect}
          custom={value.shader?.name ?? null}
          onChange={(effect) => onChange(effect === CUSTOM && !value.shader ? { effect, shader: { ...DEFAULT_SHADER } } : { effect })}
        />
        <AnimatePresence initial={false}>
          {value.effect === CUSTOM && value.shader && <ShaderEditor key="shader" value={value.shader} onChange={(shader) => onChange({ shader })} />}
        </AnimatePresence>
        <AnimatePresence initial={false}>
          {value.effect !== "none" && (
            <motion.div
              initial={{ opacity: 0, y: -6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={SPRING}
              className="flex flex-col gap-2 overflow-hidden"
            >
              <Labeled label="Strength" shown={`${value.intensity}%`}><Slider label="Strength" value={value.intensity} min={LIMITS.intensity[0]} max={LIMITS.intensity[1]} format={(n) => `${n}%`} onChange={(intensity) => onChange({ intensity })} className="pt-6" /></Labeled>
              {moves(value.effect) && (
                <Labeled label="Speed" shown={value.speed === 0 ? "Still" : `${value.speed}%`}><Slider label="Speed" value={value.speed} min={LIMITS.speed[0]} max={LIMITS.speed[1]} step={10} format={(n) => (n === 0 ? "Still" : `${n}%`)} onChange={(speed) => onChange({ speed })} className="pt-6" /></Labeled>
              )}
            </motion.div>
          )}
        </AnimatePresence>
      </Block>

      <Block title="Panels" hint="How solid the sidebar and chat are over the backdrop. More solid reads better.">
        <Labeled label="Panels" shown={`${value.panels}%`}><Slider label="Panels" value={value.panels} min={LIMITS.panels[0]} max={LIMITS.panels[1]} format={(n) => `${n}%`} onChange={(panels) => onChange({ panels })} className="pt-6" /></Labeled>
      </Block>
    </div>
  );
}

/** A slider with its name and value beside it. */
export function Labeled({ label, shown, children }: { label: string; shown: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[5rem_1fr_3rem] items-center gap-3">
      <span className="pt-5 text-sm font-bold">{label}</span>
      {children}
      <span className="pt-5 text-right text-xs font-bold text-muted-foreground tabular-nums">{shown}</span>
    </div>
  );
}

function Block({ title, hint, children }: { title: string; hint: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-3">
      <div>
        <p className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{title}</p>
        <p className="mt-0.5 text-xs text-muted-foreground">{hint}</p>
      </div>
      {children}
    </div>
  );
}

/** Effect cards, each showing its effect in miniature (the light CSS version, so a grid of them costs nothing). */
function EffectGrid({ value, custom, onChange }: { value: Effect; custom: string | null; onChange: (effect: Effect) => void }) {
  return (
    <div role="radiogroup" className="grid grid-cols-3 gap-2 sm:grid-cols-5">
      {EFFECTS.map((effect, n) => {
        const active = effect === value;
        return (
          <motion.button
            key={effect}
            type="button"
            role="radio"
            aria-checked={active}
            title={EFFECT_INFO[effect].hint}
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: n * 0.025 } }}
            whileHover={{ y: -2 }}
            whileTap={{ scale: 0.95 }}
            onClick={() => onChange(effect)}
            className={cn("group relative flex flex-col overflow-hidden rounded-xl border text-left transition-colors", active ? "border-primary" : "hover:border-primary/40")}
          >
            <span className="backdrop-layers relative! h-14 bg-background">
              {effect === CUSTOM ? (
                <span className="grid size-full place-items-center overflow-hidden">
                  <span className={cn("fx-custom-thumb", active && "on")} />
                  <CodeXmlIcon className="relative size-5 text-primary" />
                </span>
              ) : effect !== "none" &&
                (isShader(effect) ? (
                  <span className={cn("fx-css", `fx-${effect}`, !active && "still")} style={{ opacity: 0.9 }}>
                    {effect === "petals" && Array.from({ length: 14 }, (_, i) => <span key={i} />)}
                  </span>
                ) : (
                  <span className={cn("fx-texture", `fx-${effect}`)} />
                ))}
            </span>
            <span className="flex items-center justify-between gap-1 px-2 py-1.5 text-xs font-bold">
              <span className="truncate">{effect === CUSTOM && custom ? custom : EFFECT_INFO[effect].name}</span>
              {active && (
                <motion.span layoutId="effect-check" transition={SPRING} className="grid size-4 place-items-center rounded-full bg-primary text-primary-foreground">
                  <CheckIcon className="size-3" />
                </motion.span>
              )}
            </span>
          </motion.button>
        );
      })}
    </div>
  );
}

type Uploading = { preview: string; sent: number };

/** Your backgrounds on this instance, a tile to add one (click or drop), and None. */
function PictureLibrary({ instanceKey, value, onChange }: { instanceKey?: string; value: string; onChange: (url: string) => void }) {
  const [pictures, setPictures] = useState<string[] | null>(null);
  const [uploading, setUploading] = useState<Uploading | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [confirming, setConfirming] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!instanceKey) return;
    let live = true;
    run(listBackgrounds(instanceKey))
      .then((list) => live && setPictures(list.map((m) => m.url)))
      .catch(() => live && setPictures([]));
    return () => {
      live = false;
    };
  }, [instanceKey]);

  // The one in use shows even when it lives on another instance.
  const kept = new Set(pictures);
  const shown = [...(value && !kept.has(value) ? [value] : []), ...(pictures ?? [])];

  async function add(file: File | undefined) {
    if (!file || !instanceKey || uploading) return;
    setError(null);
    if (!PICTURE_TYPES.includes(file.type)) return setError("Pick a PNG, JPEG, GIF, WebP or AVIF picture.");
    const picture = await backgroundPicture(file);
    const preview = URL.createObjectURL(picture);
    setUploading({ preview, sent: 0 });
    try {
      const url = await run(uploadPicture(instanceKey, MediaPurpose.BACKGROUND, picture, (sent) => setUploading((u) => u && { ...u, sent })));
      const kept = await run(keepBackground(instanceKey, url));
      setPictures((list) => [kept.url, ...(list ?? []).filter((u) => u !== kept.url)]);
      onChange(kept.url);
    } catch (err) {
      setError(((err as Error).message || "the upload didn't go through").replace(/^./, (c) => c.toUpperCase()));
    } finally {
      setUploading(null);
      URL.revokeObjectURL(preview);
    }
  }

  async function remove(url: string) {
    if (!instanceKey) return;
    if (confirming !== url) return setConfirming(url);
    setConfirming(null);
    try {
      await run(deleteBackground(instanceKey, url));
      setPictures((list) => (list ?? []).filter((u) => u !== url));
      if (value === url) onChange("");
    } catch (err) {
      setError((err as Error).message);
    }
  }

  const drop = {
    onDragOver: (e: DragEvent) => {
      if (!instanceKey || !e.dataTransfer.types.includes("Files")) return;
      e.preventDefault();
      setDragging(true);
    },
    onDragLeave: (e: DragEvent) => !e.currentTarget.contains(e.relatedTarget as Node | null) && setDragging(false),
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      setDragging(false);
      void add(e.dataTransfer.files[0]);
    },
  };

  return (
    <div className="flex flex-col gap-2" {...drop}>
      <div className="grid grid-cols-3 gap-2 sm:grid-cols-4">
        <Tile active={!value} onClick={() => onChange("")} label="No picture">
          <span className="grid size-full place-items-center bg-muted text-muted-foreground">
            <ImageOffIcon className="size-5" />
          </span>
        </Tile>
        {instanceKey && (
          <motion.button
            type="button"
            onClick={() => input.current?.click()}
            disabled={!!uploading}
            animate={dragging ? { scale: 1.05 } : { scale: 1 }}
            whileHover={{ y: -2 }}
            whileTap={{ scale: 0.95 }}
            transition={SPRING}
            className={cn(
              "relative grid aspect-video place-items-center overflow-hidden rounded-xl border-2 border-dashed text-muted-foreground transition-colors hover:border-primary/60 hover:text-primary",
              dragging && "border-primary text-primary",
            )}
          >
            {uploading ? (
              <>
                <img src={uploading.preview} alt="" className="absolute inset-0 size-full object-cover" style={{ filter: `blur(${(1 - uploading.sent) * 6}px)`, opacity: 0.4 + uploading.sent * 0.6 }} />
                <span className="relative flex items-center gap-1 rounded-full bg-background/85 px-2 py-0.5 text-xs font-bold text-foreground">
                  <Loader2Icon className="size-3 animate-spin" />
                  {Math.round(uploading.sent * 100)}%
                </span>
              </>
            ) : (
              <span className="flex flex-col items-center gap-1 text-[0.65rem] font-extrabold tracking-wide uppercase">
                <motion.span animate={dragging ? { y: [0, -3, 0] } : { y: 0 }} transition={{ duration: 0.8, repeat: dragging ? Infinity : 0 }}>
                  <ImagePlusIcon className="size-5" />
                </motion.span>
                {dragging ? "Drop it" : "Add"}
              </span>
            )}
          </motion.button>
        )}
        <AnimatePresence initial={false}>
          {shown.map((url) => {
            const src = shownPicture(url);
            return (
              <motion.div key={url} layout initial={{ opacity: 0, scale: 0.8 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.8 }} transition={SPRING} className="group relative">
                <Tile active={value === url} onClick={() => onChange(url)} label="Use this picture">
                  {src ? (
                    <img src={src} alt="" loading="lazy" draggable={false} className="size-full object-cover transition-transform duration-500 group-hover:scale-105" />
                  ) : (
                    <span className="grid size-full place-items-center bg-muted text-[0.6rem] text-muted-foreground">Not on your instances</span>
                  )}
                </Tile>
                {kept.has(url) && (
                  <button
                    type="button"
                    onClick={() => void remove(url)}
                    onBlur={() => setConfirming(null)}
                    title="Delete this background from your instance"
                    className={cn(
                      "absolute top-1 right-1 flex items-center gap-1 rounded-full bg-background/90 px-1.5 py-1 text-[0.65rem] font-bold text-destructive opacity-0 shadow transition-opacity group-hover:opacity-100 focus-visible:opacity-100",
                      confirming === url && "opacity-100",
                    )}
                  >
                    <Trash2Icon className="size-3" />
                    {confirming === url && "Delete?"}
                  </button>
                )}
              </motion.div>
            );
          })}
        </AnimatePresence>
      </div>
      {!instanceKey && <p className="text-xs text-muted-foreground">Open an instance you're signed in to, to upload pictures to it.</p>}
      <AnimatePresence>
        {error && (
          <motion.p initial={{ opacity: 0, y: -4 }} animate={{ opacity: 1, y: 0, x: [0, -6, 5, -3, 0] }} exit={{ opacity: 0 }} className="text-xs font-bold text-destructive">
            {error}
          </motion.p>
        )}
      </AnimatePresence>
      <input ref={input} type="file" accept={PICTURE_TYPES.join(",")} hidden onChange={(e) => (void add(e.target.files?.[0]), (e.target.value = ""))} />
    </div>
  );
}

function Tile({ active, onClick, label, children }: { active: boolean; onClick: () => void; label: string; children: ReactNode }) {
  return (
    <motion.button
      type="button"
      aria-label={label}
      aria-pressed={active}
      whileHover={{ y: -2 }}
      whileTap={{ scale: 0.95 }}
      onClick={onClick}
      className={cn("relative block aspect-video w-full overflow-hidden rounded-xl border-2 transition-colors", active ? "border-primary" : "border-transparent hover:border-primary/40")}
    >
      {children}
      {active && (
        <motion.span layoutId="picture-check" transition={SPRING} className="absolute bottom-1 left-1 grid size-5 place-items-center rounded-full bg-primary text-primary-foreground shadow">
          <CheckIcon className="size-3" />
        </motion.span>
      )}
    </motion.button>
  );
}
