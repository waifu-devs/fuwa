import { DownloadIcon, EyeIcon, LoaderCircleIcon, LockKeyholeIcon, PlayIcon, ShieldAlertIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { memo, useCallback, useEffect, useRef, useState } from "react";
import { FileBadge } from "@/components/chat/Attachments";
import { FirstFrame } from "@/components/chat/GifImage";
import { fitGif } from "@/components/chat/GifMessage";
import { ImageViewer, type ViewerPicture } from "@/components/chat/ImageViewer";
import { GifProvider } from "@/gen/fuwa/v1/types_pb";
import { providerName } from "@/fuwa/gifs";
import { SpoilerBadge, SpoilerCover } from "@/components/chat/SpoilerCover";
import type { FileRef } from "@/e2ee/vault";
import { animated, sniff, type Preview } from "@/files/sealed";
import { openDmFile } from "@/fuwa/dms";
import { fitBox, shortName } from "@/lib/attachments";
import { formatBytes } from "@/lib/format";
import { useI18n } from "@/i18n/react";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { cn } from "@/lib/utils";

/** The most room one picture or video takes in a message. */
const MEDIA_BOX = { width: 420, height: 320 };
/** Pictures and videos up to this size open by themselves once they're on screen. */
const AUTO_OPEN_BYTES = 25 * 1024 * 1024;

/** A file once opened on this device, and what it can be shown as. */
type Opened = { blob: Blob; preview: Preview; moves: boolean };

type State = { at: "idle" } | { at: "opening" } | { at: "open"; opened: Opened } | { at: "failed"; problem: string };

async function openShown(instanceKey: string, file: FileRef): Promise<Opened> {
  const blob = await openDmFile(instanceKey, file);
  const head = new Uint8Array(await blob.slice(0, 64).arrayBuffer());
  const preview = sniff(head);
  return { blob, preview, moves: !!preview && animated(head, preview.type) };
}

/** Saves an opened file: always as plain bytes with its cleaned name, never opened in a tab. */
function save(blob: Blob, name: string) {
  const url = URL.createObjectURL(new Blob([blob], { type: "application/octet-stream" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.rel = "noopener";
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/** A blob URL for an opened picture or video, typed by its own bytes, revoked when it's off screen. */
function useBlobUrl(opened: Opened | null): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!opened?.preview) return;
    const next = URL.createObjectURL(new Blob([opened.blob], { type: opened.preview.type }));
    setUrl(next);
    return () => {
      URL.revokeObjectURL(next);
      setUrl(null);
    };
  }, [opened]);
  return url;
}

/**
 * The files an encrypted message carries. Each is fetched from this
 * instance and opened on this device; pictures and videos show inline once
 * opened (small ones by themselves, when they come on screen), and only
 * when their own bytes say they're a type that's safe to draw. Everything
 * else is a card to download. A file marked as a spoiler stays covered until
 * it's clicked (it may still open behind the cover, so uncovering is quick).
 * A shown picture opens in the picture viewer on a click, which steps through
 * the message's other pictures opened here (and uncovered, for spoilers);
 * they're never opened in a tab, so it has no "open the original".
 */
export const SealedFiles = memo(function SealedFiles({ instanceKey, files, animate }: { instanceKey: string; files: FileRef[]; animate: boolean }) {
  const [viewable, setViewable] = useState<Record<string, ViewerPicture>>({});
  const [open, setOpen] = useState<number | null>(null);
  const report = useCallback((mediaId: string, picture: ViewerPicture | null) => {
    setViewable((was) => {
      if (picture ? was[mediaId]?.src === picture.src : !was[mediaId]) return was;
      const next = { ...was };
      if (picture) next[mediaId] = picture;
      else delete next[mediaId];
      return next;
    });
  }, []);
  if (!files.length) return null;
  const pictures = files.flatMap((f) => viewable[f.mediaId] ?? []);
  return (
    <div className="mt-1 flex flex-col gap-1.5">
      {files.map((file, n) =>
        file.gif ? (
          <SealedGif key={file.mediaId} instanceKey={instanceKey} file={file} animate={animate} />
        ) : (
          <SealedFile
            key={file.mediaId}
            instanceKey={instanceKey}
            file={file}
            delay={animate ? n * 0.04 : 0}
            animate={animate}
            onViewable={report}
            onView={() => setOpen(Math.max(0, pictures.findIndex((p) => p.key === file.mediaId)))}
          />
        ),
      )}
      <ImageViewer pictures={pictures} index={pictures.length ? open : null} onIndex={setOpen} onClose={() => setOpen(null)} />
    </div>
  );
});

/**
 * Opening one file on this device, once: by hand, or by itself when `auto`
 * and its place (`place`) comes on screen.
 */
function useOpening(instanceKey: string, file: FileRef, auto: boolean) {
  const lang = useI18n();
  const [state, setState] = useState<State>({ at: "idle" });
  const place = useRef<HTMLDivElement>(null);

  const open = async (): Promise<Opened | null> => {
    if (state.at === "open") return state.opened;
    setState({ at: "opening" });
    try {
      const next = await openShown(instanceKey, file);
      setState({ at: "open", opened: next });
      return next;
    } catch (err) {
      setState({ at: "failed", problem: err instanceof Error ? err.message : lang.t("dms-calls.dm.sealed.cantOpen") });
      return null;
    }
  };

  // Small pictures and videos open once they're on screen, not before.
  useEffect(() => {
    const el = place.current;
    if (!auto || state.at !== "idle" || !el) return;
    const seen = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        seen.disconnect();
        void open();
      }
    });
    seen.observe(el);
    return () => seen.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auto, state.at]);

  return { state, open, place };
}

function SealedFile({
  instanceKey,
  file,
  delay,
  animate,
  onViewable,
  onView,
}: {
  instanceKey: string;
  file: FileRef;
  delay: number;
  animate: boolean;
  /** Says whether it's a picture the viewer can show now (opened, and uncovered if it's a spoiler). */
  onViewable: (mediaId: string, picture: ViewerPicture | null) => void;
  onView: () => void;
}) {
  const lang = useI18n();
  const hinted = file.type.startsWith("image/") || file.type.startsWith("video/") || file.width > 0;
  const auto = hinted && (file.fileSize || file.size) <= AUTO_OPEN_BYTES;
  const { state, open, place } = useOpening(instanceKey, file, auto);
  const opened = state.at === "open" ? state.opened : null;
  const url = useBlobUrl(opened);
  const [uncovered, setUncovered] = useState(false);
  const picture = !!url && opened?.preview?.kind !== "video" && (!file.spoiler || uncovered);

  useEffect(() => {
    if (!picture || !url || !opened) return onViewable(file.mediaId, null);
    onViewable(file.mediaId, {
      key: file.mediaId,
      src: url,
      name: file.name,
      bytes: opened.blob.size,
      width: file.width,
      height: file.height,
      save: () => save(opened.blob, file.name),
    });
    return () => onViewable(file.mediaId, null);
  }, [picture, url, opened, file, onViewable]);

  const download = async () => {
    const o = await open();
    if (o) save(o.blob, file.name);
  };

  const enterWith = animate
    ? { initial: { opacity: 0, y: 6, scale: 0.97 }, animate: { opacity: 1, y: 0, scale: 1 }, transition: { type: "spring", stiffness: 460, damping: 30, delay } as const }
    : {};

  if (opened?.preview && url) {
    const size = fitBox(file.width, file.height, MEDIA_BOX);
    // The same cover as the card's below, so uncovering one stays uncovered once it opens.
    return (
      <SpoilerCover spoiler={!!file.spoiler} look="media" animate={animate} className="self-start rounded-xl" onShown={() => setUncovered(true)}>
        <motion.div {...enterWith} className="group/sealed relative max-w-full" style={{ width: size.width }}>
          <Shown file={file} opened={opened} url={url} size={size} onView={onView} />
          <button
            type="button"
            onClick={() => save(opened.blob, file.name)}
            aria-label={lang.t("dms-calls.dm.sealed.download", { name: file.name })}
            title={lang.t("dms-calls.dm.sealed.download", { name: file.name })}
            className="absolute top-2 right-2 grid size-8 place-items-center rounded-lg bg-black/55 text-white opacity-0 shadow transition group-hover/sealed:opacity-100 hover:scale-110 focus-visible:opacity-100"
          >
            <DownloadIcon className="size-4" />
          </button>
        </motion.div>
      </SpoilerCover>
    );
  }

  const size = opened ? opened.blob.size : file.fileSize;
  const busy = state.at === "opening";
  return (
    <SpoilerCover spoiler={!!file.spoiler} look="card" animate={animate} className="w-full max-w-sm rounded-xl">
      <motion.div
        ref={place}
        {...enterWith}
        className={cn("flex w-full max-w-sm items-center gap-3 rounded-xl border bg-muted/40 px-3 py-2.5", state.at === "failed" && "border-destructive/50")}
      >
        <span className="relative">
          <FileBadge name={file.name} />
          <span className="absolute -right-1 -bottom-1 grid size-4 place-items-center rounded-full bg-card text-emerald-500 shadow-sm" title={lang.t("dms-calls.dm.sealed.encrypted")}>
            <LockKeyholeIcon className="size-2.5" />
          </span>
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-bold" title={file.name}>
            {shortName(file.name, 48)}
          </p>
          <AnimatePresence mode="wait" initial={false}>
            <motion.p
              key={state.at}
              initial={{ opacity: 0, y: 3 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -3 }}
              transition={{ duration: 0.15 }}
              className={cn("truncate text-xs tabular-nums", state.at === "failed" ? "text-destructive" : "text-muted-foreground")}
            >
              <Status state={state} size={size} />
            </motion.p>
          </AnimatePresence>
        </div>
        {hinted && !opened && (
          <CardButton label={lang.t("dms-calls.dm.sealed.show", { name: file.name })} onClick={() => void open()} disabled={busy}>
            {busy ? <LoaderCircleIcon className="size-4 animate-spin" /> : <EyeIcon className="size-4" />}
          </CardButton>
        )}
        <CardButton label={lang.t("dms-calls.dm.sealed.download", { name: file.name })} onClick={() => void download()} disabled={busy}>
          <DownloadIcon className="size-4" />
        </CardButton>
      </motion.div>
    </SpoilerCover>
  );
}

const noop = () => {};

/**
 * A GIF sent from the picker, sealed as a file: opened by itself once it's on
 * screen and shown like a GIF in a channel (sized before it opens, playing
 * inline, still under reduce motion until it's pointed at or tapped, with
 * its credit). Whatever its own bytes don't say is a picture shows as a file.
 */
function SealedGif({ instanceKey, file, animate }: { instanceKey: string; file: FileRef; animate: boolean }) {
  const { t } = useI18n();
  const still = usePrefs(reduceMotion);
  const [hover, setHover] = useState(false);
  const [tapped, setTapped] = useState(false);
  const { state, place } = useOpening(instanceKey, file, (file.fileSize || file.size) <= AUTO_OPEN_BYTES);
  const opened = state.at === "open" ? state.opened : null;
  const url = useBlobUrl(opened);
  if (state.at === "failed" || (opened && opened.preview?.kind !== "image")) {
    return <SealedFile instanceKey={instanceKey} file={file} delay={0} animate={animate} onViewable={noop} onView={noop} />;
  }
  const size = fitGif(file.width || 240, file.height || 180);
  const playing = !still || hover || tapped;
  const alt = file.gif?.title || t("chat.gif.gif");
  const credit = providerName(file.gif?.provider ?? GifProvider.UNSPECIFIED);
  return (
    <motion.div
      ref={place}
      initial={animate ? { opacity: 0, scale: 0.94, y: 6 } : false}
      animate={{ opacity: 1, scale: 1, y: 0 }}
      transition={{ type: "spring", stiffness: 460, damping: 30 }}
      style={{ width: size.width, height: size.height }}
      onPointerEnter={() => setHover(true)}
      onPointerLeave={() => setHover(false)}
      className="group/gif relative max-w-full overflow-hidden rounded-xl bg-muted shadow-sm"
    >
      {url ? (
        <button
          type="button"
          onClick={() => still && setTapped((on) => !on)}
          onFocus={() => setHover(true)}
          onBlur={() => setHover(false)}
          aria-label={alt}
          className={cn("block size-full", still ? "cursor-pointer" : "cursor-default")}
        >
          {playing || !opened?.moves ? (
            <img src={url} alt={alt} draggable={false} decoding="async" className="size-full object-cover" />
          ) : (
            <FirstFrame src={url} alt={alt} className="size-full object-cover" />
          )}
        </button>
      ) : (
        <span className="grid size-full place-items-center text-muted-foreground">
          <LoaderCircleIcon className="size-5 animate-spin" aria-label={t("dms-calls.dm.sealed.opening")} />
        </span>
      )}
      <AnimatePresence>
        {url && still && !playing && opened?.moves && (
          <motion.span
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="pointer-events-none absolute bottom-2 left-2 rounded-md bg-black/60 px-1.5 py-0.5 text-[0.65rem] font-extrabold tracking-wide text-white"
          >
            {t("chat.gif.gif")}
          </motion.span>
        )}
      </AnimatePresence>
      {credit && (
        <span className="pointer-events-none absolute right-2 bottom-1.5 text-[0.6rem] font-bold text-white/85 opacity-0 transition-opacity duration-150 [text-shadow:0_1px_2px_rgb(0_0_0/0.6)] group-hover/gif:opacity-100">
          {t("chat.gif.via", { provider: credit })}
        </span>
      )}
    </motion.div>
  );
}

/** An opened picture or video; a moving picture holds still under reduce motion. A picture opens in the viewer on a click. */
function Shown({ file, opened, url, size, onView }: { file: FileRef; opened: Opened; url: string; size: { width: number; height: number }; onView: () => void }) {
  const { t } = useI18n();
  usePrefs((p) => p.reduceMotion);
  const calm = reduceMotion();
  if (opened.preview?.kind === "video") {
    return <video src={url} controls playsInline preload="metadata" className="max-h-80 w-full rounded-xl border bg-black" aria-label={file.name} />;
  }
  if (opened.moves && calm) return <Still url={url} name={file.name} width={size.width} height={size.height} onView={onView} />;
  return (
    <button type="button" onClick={onView} aria-label={t("chat.attachments.open", { name: file.name })} className="block w-full cursor-zoom-in">
      <img
        src={url}
        alt={file.name}
        draggable={false}
        style={{ aspectRatio: file.width > 0 && file.height > 0 ? `${size.width} / ${size.height}` : undefined }}
        className="w-full rounded-xl border bg-muted/60 object-contain"
      />
    </button>
  );
}

/** The card's second line: why it failed, that it's opening, or how big it is. */
function Status({ state, size }: { state: State; size: number }) {
  const lang = useI18n();
  if (state.at === "failed") {
    return (
      <span className="inline-flex items-center gap-1 first-letter:uppercase">
        <ShieldAlertIcon className="size-3" />
        {state.problem}
      </span>
    );
  }
  if (state.at === "opening") return lang.t("dms-calls.dm.sealed.opening");
  return size > 0 ? formatBytes(lang, size) : lang.t("dms-calls.dm.sealed.file");
}

function CardButton({ label, onClick, disabled, children }: { label: string; onClick: () => void; disabled?: boolean; children: React.ReactNode }) {
  return (
    <motion.button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      title={label}
      whileHover={{ scale: 1.1 }}
      whileTap={{ scale: 0.9 }}
      className="grid size-8 shrink-0 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-primary/10 hover:text-primary disabled:opacity-50"
    >
      {children}
    </motion.button>
  );
}

/**
 * A moving picture under reduce motion: its first frame, drawn once, until
 * it's clicked; once moving, a click opens it in the viewer.
 */
function Still({ url, name, width, height, onView }: { url: string; name: string; width: number; height: number; onView: () => void }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [playing, setPlaying] = useState(false);
  const { t } = useI18n();
  useEffect(() => {
    if (playing) return;
    const img = new Image();
    img.onload = () => {
      const c = canvas.current;
      if (!c) return;
      c.width = img.naturalWidth;
      c.height = img.naturalHeight;
      c.getContext("2d")?.drawImage(img, 0, 0);
    };
    img.src = url;
  }, [url, playing]);
  if (playing) {
    return (
      <button type="button" onClick={onView} aria-label={t("chat.attachments.open", { name })} className="block w-full cursor-zoom-in">
        <img src={url} alt={name} draggable={false} className="w-full rounded-xl border bg-muted/60 object-contain" />
      </button>
    );
  }
  return (
    <button
      type="button"
      onClick={() => setPlaying(true)}
      aria-label={t("dms-calls.dm.sealed.play", { name })}
      className="relative block w-full overflow-hidden rounded-xl border bg-muted/60"
      style={{ aspectRatio: `${width} / ${height}` }}
    >
      <canvas ref={canvas} className="size-full object-contain" />
      <span className="absolute inset-0 grid place-items-center">
        <span className="grid size-10 place-items-center rounded-full bg-black/55 text-white">
          <PlayIcon className="size-5" />
        </span>
      </span>
    </button>
  );
}

/**
 * Files on their way in an encrypted message: sealed and uploaded on this
 * device before it's sent. One marked as a spoiler says so, its name blurred.
 */
export function PendingFiles({ files }: { files: { name: string; size: number; spoiler?: boolean; gif?: boolean }[] }) {
  const lang = useI18n();
  return (
    <div className="mt-1 flex flex-col gap-1.5">
      {files.map((file, n) => (
        <div key={n} className="relative flex w-full max-w-sm items-center gap-3 overflow-hidden rounded-xl border bg-muted/40 px-3 py-2.5">
          <FileBadge name={file.name} />
          <div className="min-w-0 flex-1">
            <p className={cn("truncate text-sm font-bold", file.spoiler && "blur-[4px] select-none")}>{file.gif ? lang.t("chat.gif.gif") : shortName(file.name, 48)}</p>
            <p className="truncate text-xs text-muted-foreground tabular-nums">{formatBytes(lang, file.size)}</p>
          </div>
          {file.spoiler && <SpoilerBadge className="shrink-0 px-2 py-0.5 text-[0.6rem]" />}
          <LockKeyholeIcon className="size-4 shrink-0 text-emerald-500" />
        </div>
      ))}
    </div>
  );
}
