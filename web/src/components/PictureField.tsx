import { CameraIcon, CheckIcon, ImageUpIcon, LinkIcon, Trash2Icon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useId, useRef, useState, type DragEvent, type ReactNode } from "react";
import { MediaPurpose } from "@/gen/fuwa/v1/media_pb";
import { run, uploadPicture } from "@/fuwa/actions";
import { SPRING } from "@/lib/motion";
import { PictureCropper } from "@/components/PictureCropper";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { PICTURE_TYPES, type PictureKind } from "@/lib/pictures";
import { shownPicture } from "@/lib/shown";
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

const PURPOSE: Record<PictureKind, MediaPurpose> = {
  avatar: MediaPurpose.AVATAR,
  banner: MediaPurpose.BANNER,
  icon: MediaPurpose.SERVER_ICON,
};

const TILE: Record<PictureKind, string> = {
  avatar: "size-20 rounded-full",
  banner: "h-24 w-60 rounded-2xl",
  icon: "size-20 rounded-[32%]",
};

/** The dashed ring that shows while a file is dragged over, shaped like the picture. */
const DROP_RING: Record<PictureKind, string> = {
  avatar: "rounded-full",
  banner: "rounded-xl",
  icon: "rounded-[30%]",
};

const isLink = (value: string) => !value.trim() || /^https?:\/\/\S+$/i.test(value.trim());

type Upload = { preview: string; sent: number; done: boolean };

/** A link for a picked file while it's being framed; it's let go once the file is. */
function useObjectUrl(file: Blob | null) {
  const [made, setMade] = useState<{ file: Blob; url: string } | null>(null);
  useEffect(() => {
    if (!file) return;
    const url = URL.createObjectURL(file);
    setMade({ file, url });
    return () => URL.revokeObjectURL(url);
  }, [file]);
  return made && made.file === file ? made.url : null;
}

/** Sending a picture: its progress, what went wrong (with a shake), and dropping it if the field goes away. */
function usePictureUpload({
  instanceKey,
  kind,
  serverId,
  onUploaded,
}: {
  instanceKey: string;
  kind: PictureKind;
  serverId?: string;
  onUploaded: (url: string) => void;
}) {
  const [upload, setUpload] = useState<Upload | null>(null);
  const [error, setError] = useState<string | null>(null);
  const shake = useAnimationControls();
  const cancel = useRef<(() => void) | null>(null);
  const { t } = useI18n();

  useEffect(() => () => cancel.current?.(), []);

  function fail(message: string) {
    setError(message);
    void shake.start({ x: [0, -8, 7, -5, 3, 0], transition: { duration: 0.4 } });
  }

  async function send(picture: Blob) {
    const preview = URL.createObjectURL(picture);
    setUpload({ preview, sent: 0, done: false });
    let live = true;
    cancel.current = () => {
      live = false;
    };
    try {
      const url = await run(uploadPicture(instanceKey, PURPOSE[kind], picture, (sent) => live && setUpload((u) => u && { ...u, sent }), serverId));
      if (!live) return;
      setUpload((u) => u && { ...u, sent: 1, done: true });
      onUploaded(url);
      setTimeout(() => {
        setUpload(null);
        URL.revokeObjectURL(preview);
      }, 1400);
    } catch (err) {
      if (!live) return;
      setUpload(null);
      URL.revokeObjectURL(preview);
      fail(((err as Error).message || t("workspace.picture.uploadFailed")).replace(/^./, (c) => c.toUpperCase()));
    }
  }

  return { upload, error, setError, fail, shake, send };
}

/** Dragging a file over the field and dropping it there. */
function dropHandlers(blocked: boolean, setDragging: (on: boolean) => void, choose: (file: File | undefined) => void) {
  return {
    onDragOver: (e: DragEvent) => {
      if (blocked || !e.dataTransfer.types.includes("Files")) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "copy";
      setDragging(true);
    },
    onDragLeave: (e: DragEvent) => {
      if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
    },
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      setDragging(false);
      choose(e.dataTransfer.files[0]);
    },
  };
}

/**
 * A picture people set by uploading one: click or drop a file, frame it (GIFs
 * go up as they are, so they keep moving), and watch it upload. It can also
 * be removed, or set from a link. `onChange` gets the new link; saving it is
 * up to the page, and uploads nothing ends up using are cleared by the server.
 */
export function PictureField({
  instanceKey,
  kind,
  value,
  onChange,
  fallback,
  disabled = false,
  compact = false,
  id,
  serverId,
}: {
  instanceKey: string;
  kind: PictureKind;
  value: string;
  onChange: (url: string) => void;
  /** What shows with no picture: initials, a color. */
  fallback: ReactNode;
  disabled?: boolean;
  /** Just the picture, with a camera badge: for tight spots like making a server. */
  compact?: boolean;
  id?: string;
  /** The server it's for: its icon, or one of its webhooks' pictures. */
  serverId?: string;
}) {
  const input = useRef<HTMLInputElement>(null);
  const inputId = useId();
  const [picked, setPicked] = useState<File | null>(null);
  const pickedUrl = useObjectUrl(picked);
  const [dragging, setDragging] = useState(false);
  const [linking, setLinking] = useState(false);
  // The link that failed to load; a new one gets its own try.
  const [brokenLink, setBrokenLink] = useState<string | null>(null);
  const { t } = useI18n();
  const { upload, error, setError, fail, shake, send } = usePictureUpload({
    instanceKey,
    kind,
    serverId,
    onUploaded: (url) => {
      onChange(url);
      setLinking(false);
    },
  });
  const busy = !!upload && !upload.done;

  function choose(file: File | undefined) {
    if (!file || disabled || busy) return;
    setError(null);
    if (!PICTURE_TYPES.includes(file.type)) return fail(t("workspace.picture.wrongType"));
    // A GIF would stop moving if it were redrawn, so it goes up as it is.
    if (file.type === "image/gif") return void send(file);
    setPicked(file);
  }

  const browse = () => input.current?.click();
  // A link that isn't one stays open so it can be fixed.
  const showLink = !compact && (linking || !isLink(value));
  const tile = (
    <PictureTile
      id={id}
      kind={kind}
      hasPicture={!!value}
      shown={upload?.preview ?? shownLink(value, brokenLink)}
      fallback={fallback}
      upload={upload}
      disabled={disabled}
      dragging={dragging}
      onClick={browse}
      onBroken={() => !upload && setBrokenLink(value)}
    />
  );

  return (
    <div className="flex flex-col gap-2" {...dropHandlers(disabled || busy, setDragging, choose)}>
      <input
        ref={input}
        id={inputId}
        type="file"
        accept={PICTURE_TYPES.join(",")}
        aria-label={t(`workspace.picture.upload.${kind}`)}
        className="sr-only"
        tabIndex={-1}
        onChange={(e) => {
          choose(e.target.files?.[0]);
          e.target.value = "";
        }}
      />
      <motion.div animate={shake} className={cn("flex items-center gap-4", compact && "justify-center")}>
        {compact ? <CompactTile>{tile}</CompactTile> : tile}
        {!compact && (
          <PictureActions
            value={value}
            disabled={disabled}
            busy={busy}
            showLink={showLink}
            onBrowse={browse}
            onRemove={() => {
              onChange("");
              setError(null);
            }}
            onToggleLink={() => setLinking((l) => !l)}
          />
        )}
      </motion.div>

      <AnimatePresence initial={false}>
        {showLink && (
          <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
            <LinkInput kind={kind} value={value} disabled={disabled || busy} onChange={onChange} />
          </motion.div>
        )}
      </AnimatePresence>

      <AnimatePresence initial={false}>
        {error && (
          <motion.p
            role="alert"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            className={cn("overflow-hidden text-sm font-bold text-destructive", compact && "text-center")}
          >
            {error}
          </motion.p>
        )}
      </AnimatePresence>

      <PictureCropper
        src={pickedUrl}
        kind={kind}
        onCancel={() => setPicked(null)}
        onDone={(picture) => {
          setPicked(null);
          void send(picture);
        }}
      />
    </div>
  );
}

/**
 * The saved picture, when it can show. A typed link to another site isn't
 * loaded (it would learn this person's address): only pictures on a trusted
 * instance show before saving.
 */
function shownLink(value: string, brokenLink: string | null) {
  if (!value || !isLink(value) || brokenLink === value) return null;
  return shownPicture(value.trim()) || null;
}

/** The picture with a camera badge, for compact fields. */
function CompactTile({ children }: { children: ReactNode }) {
  return (
    <span className="relative">
      {children}
      <motion.span
        initial={{ scale: 0 }}
        animate={{ scale: 1 }}
        transition={{ ...SPRING, delay: 0.15 }}
        className="pointer-events-none absolute -right-1 -bottom-1 grid size-7 place-items-center rounded-full border-2 border-card bg-primary text-primary-foreground shadow"
      >
        <CameraIcon className="size-3.5" />
      </motion.span>
    </span>
  );
}

/** The picture itself: click to pick a file, drop one on it, and see the upload go. */
function PictureTile({
  id,
  kind,
  hasPicture,
  shown,
  fallback,
  upload,
  disabled,
  dragging,
  onClick,
  onBroken,
}: {
  id?: string;
  kind: PictureKind;
  hasPicture: boolean;
  shown: string | null;
  fallback: ReactNode;
  upload: Upload | null;
  disabled: boolean;
  dragging: boolean;
  onClick: () => void;
  onBroken: () => void;
}) {
  const { t } = useI18n();
  const busy = !!upload && !upload.done;
  return (
    <motion.button
      type="button"
      id={id}
      aria-label={hasPicture ? t(`workspace.picture.change.${kind}`) : t(`workspace.picture.upload.${kind}`)}
      disabled={disabled || busy}
      onClick={onClick}
      animate={dragging ? { scale: 1.06 } : { scale: 1 }}
      whileTap={{ scale: 0.95 }}
      transition={SPRING}
      className={cn("group relative shrink-0 overflow-hidden border bg-muted outline-none focus-visible:ring-2 focus-visible:ring-primary", TILE[kind])}
    >
      <AnimatePresence initial={false}>
        <motion.span
          key={shown ?? "empty"}
          initial={{ opacity: 0, scale: 1.15 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.45, ease: [0.22, 1, 0.36, 1] }}
          className="absolute inset-0"
        >
          {shown ? <TileImage src={shown} sent={busy ? upload.sent : null} onBroken={onBroken} /> : fallback}
        </motion.span>
      </AnimatePresence>

      <TileHint dragging={dragging} hoverable={!busy && !disabled} />
      <AnimatePresence>
        {dragging && (
          <motion.span
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className={cn("pointer-events-none absolute inset-1 border-2 border-dashed border-white/90", DROP_RING[kind])}
          />
        )}
      </AnimatePresence>

      <AnimatePresence>{upload && <Progress kind={kind} sent={upload.sent} done={upload.done} />}</AnimatePresence>
    </motion.button>
  );
}

/** The picture, blurred and grey while it uploads (`sent` is how much has gone). */
function TileImage({ src, sent, onBroken }: { src: string; sent: number | null; onBroken: () => void }) {
  return (
    <img
      src={src}
      alt=""
      draggable={false}
      onError={onBroken}
      style={{ filter: sent === null ? undefined : `blur(${(1 - sent) * 6}px) saturate(${0.4 + sent * 0.6})` }}
      className="size-full object-cover transition-[filter] duration-300"
    />
  );
}

/** Hover and drop: what a click or a drop does. */
function TileHint({ dragging, hoverable }: { dragging: boolean; hoverable: boolean }) {
  const { t } = useI18n();
  return (
    <span
      className={cn(
        "absolute inset-0 grid place-items-center bg-black/45 text-white opacity-0 transition-opacity duration-200",
        hoverable && "group-hover:opacity-100 group-focus-visible:opacity-100",
        dragging && "opacity-100",
      )}
    >
      <span className="flex flex-col items-center gap-0.5 text-[0.65rem] font-extrabold tracking-wide uppercase">
        <motion.span animate={dragging ? { y: [0, -3, 0] } : { y: 0 }} transition={{ duration: 0.8, repeat: dragging ? Infinity : 0 }}>
          {dragging ? <ImageUpIcon className="size-5" /> : <CameraIcon className="size-5 transition-transform group-hover:scale-110" />}
        </motion.span>
        {dragging ? t("workspace.picture.dropIt") : t("workspace.picture.changeShort")}
      </span>
    </span>
  );
}

/** Beside the picture: upload, remove, and setting it from a link. */
function PictureActions({
  value,
  disabled,
  busy,
  showLink,
  onBrowse,
  onRemove,
  onToggleLink,
}: {
  value: string;
  disabled: boolean;
  busy: boolean;
  showLink: boolean;
  onBrowse: () => void;
  onRemove: () => void;
  onToggleLink: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      <Button type="button" variant="outline" disabled={disabled || busy} onClick={onBrowse} className="group rounded-xl">
        <ImageUpIcon className="transition-transform group-hover:-translate-y-0.5" />
        {value ? t("workspace.picture.changeShort") : t("workspace.picture.uploadShort")}
      </Button>
      <AnimatePresence initial={false}>
        {value && (
          <motion.span initial={{ opacity: 0, scale: 0.8 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.8 }} transition={SPRING}>
            <Button
              type="button"
              variant="ghost"
              disabled={disabled || busy}
              onClick={onRemove}
              className="group rounded-xl text-muted-foreground hover:text-destructive"
            >
              <Trash2Icon className="transition-transform group-hover:-rotate-12" /> {t("system.picture.remove")}
            </Button>
          </motion.span>
        )}
      </AnimatePresence>
      <button
        type="button"
        disabled={disabled}
        aria-expanded={showLink}
        onClick={onToggleLink}
        className="flex items-center gap-1 rounded-lg px-1.5 py-1 text-xs font-bold text-muted-foreground transition hover:text-foreground"
      >
        <LinkIcon className="size-3.5" /> {showLink ? t("workspace.picture.hideLink") : t("workspace.picture.useLink")}
      </button>
    </div>
  );
}

/** The picture's link, typed in, with a button to clear it. */
function LinkInput({ kind, value, disabled, onChange }: { kind: PictureKind; value: string; disabled: boolean; onChange: (url: string) => void }) {
  const { t } = useI18n();
  return (
    <div className="relative pt-1">
      <Input
        type="url"
        inputMode="url"
        placeholder="https://…"
        aria-label={t(`workspace.picture.link.${kind}`)}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        aria-invalid={!isLink(value) || undefined}
        className="h-11 rounded-xl pr-11"
      />
      <AnimatePresence>
        {value && (
          <motion.button
            type="button"
            initial={{ scale: 0, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            exit={{ scale: 0, opacity: 0 }}
            transition={SPRING}
            onClick={() => onChange("")}
            aria-label={t("workspace.picture.clearLink")}
            className="absolute top-[calc(50%+2px)] right-1.5 grid size-8 -translate-y-1/2 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground"
          >
            <XIcon className="size-4" />
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}

/** How much of an upload has gone: a ring around round and square pictures, a bar under banners, then a check. */
function Progress({ kind, sent, done }: { kind: PictureKind; sent: number; done: boolean }) {
  const r = 18;
  const c = 2 * Math.PI * r;
  return (
    <motion.span initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0, transition: { duration: 0.3 } }} className="pointer-events-none absolute inset-0 grid place-items-center bg-black/25 text-white">
      <AnimatePresence mode="popLayout" initial={false}>
        {done ? (
          <motion.span
            key="done"
            initial={{ scale: 0, rotate: -45 }}
            animate={{ scale: 1, rotate: 0 }}
            transition={{ type: "spring", stiffness: 600, damping: 14 }}
            className="relative grid size-9 place-items-center rounded-full bg-emerald-500 shadow-lg"
          >
            <span className="absolute inset-0 animate-ping rounded-full bg-emerald-400/70 motion-reduce:hidden" />
            <CheckIcon className="relative size-5" strokeWidth={3} />
          </motion.span>
        ) : kind === "banner" ? (
          <motion.span key="bar" className="absolute inset-x-3 bottom-3 h-1.5 overflow-hidden rounded-full bg-white/30">
            <motion.span className="block h-full rounded-full bg-white" initial={{ x: "-100%" }} animate={{ x: `${sent * 100 - 100}%` }} transition={SPRING} />
          </motion.span>
        ) : (
          <motion.svg key="ring" viewBox="0 0 44 44" className="size-11 -rotate-90 drop-shadow">
            <circle cx="22" cy="22" r={r} fill="none" stroke="currentColor" strokeOpacity={0.3} strokeWidth={4} />
            <motion.circle
              cx="22"
              cy="22"
              r={r}
              fill="none"
              stroke="currentColor"
              strokeWidth={4}
              strokeLinecap="round"
              strokeDasharray={c}
              initial={{ strokeDashoffset: c }}
              animate={{ strokeDashoffset: c * (1 - Math.max(0.04, sent)) }}
              transition={SPRING}
            />
          </motion.svg>
        )}
      </AnimatePresence>
    </motion.span>
  );
}
