import { DownloadIcon, EyeIcon, LoaderCircleIcon, LockKeyholeIcon, PlayIcon, ShieldAlertIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { memo, useEffect, useRef, useState } from "react";
import { FileBadge } from "@/components/chat/Attachments";
import type { FileRef } from "@/e2ee/vault";
import { animated, sniff, type Preview } from "@/files/sealed";
import { openDmFile } from "@/fuwa/dms";
import { fitBox, shortName } from "@/lib/attachments";
import { formatBytes } from "@/lib/format";
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
 * else is a card to download.
 */
export const SealedFiles = memo(function SealedFiles({ instanceKey, files, animate }: { instanceKey: string; files: FileRef[]; animate: boolean }) {
  if (!files.length) return null;
  return (
    <div className="mt-1 flex flex-col gap-1.5">
      {files.map((file, n) => (
        <SealedFile key={file.mediaId} instanceKey={instanceKey} file={file} delay={animate ? n * 0.04 : 0} animate={animate} />
      ))}
    </div>
  );
});

function SealedFile({ instanceKey, file, delay, animate }: { instanceKey: string; file: FileRef; delay: number; animate: boolean }) {
  const [state, setState] = useState<State>({ at: "idle" });
  const place = useRef<HTMLDivElement>(null);
  const hinted = file.type.startsWith("image/") || file.type.startsWith("video/") || file.width > 0;
  const auto = hinted && (file.fileSize || file.size) <= AUTO_OPEN_BYTES;
  const opened = state.at === "open" ? state.opened : null;
  const url = useBlobUrl(opened);
  usePrefs((p) => p.reduceMotion);
  const calm = reduceMotion();

  const open = async (): Promise<Opened | null> => {
    if (state.at === "open") return state.opened;
    setState({ at: "opening" });
    try {
      const next = await openShown(instanceKey, file);
      setState({ at: "open", opened: next });
      return next;
    } catch (err) {
      setState({ at: "failed", problem: err instanceof Error ? err.message : "this file couldn't be opened" });
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

  const download = async () => {
    const o = await open();
    if (o) save(o.blob, file.name);
  };

  const enterWith = animate
    ? { initial: { opacity: 0, y: 6, scale: 0.97 }, animate: { opacity: 1, y: 0, scale: 1 }, transition: { type: "spring", stiffness: 460, damping: 30, delay } as const }
    : {};

  if (opened?.preview && url) {
    const size = fitBox(file.width, file.height, MEDIA_BOX);
    return (
      <motion.div {...enterWith} className="group/sealed relative max-w-full" style={{ width: size.width }}>
        {opened.preview.kind === "video" ? (
          <video src={url} controls playsInline preload="metadata" className="max-h-80 w-full rounded-xl border bg-black" aria-label={file.name} />
        ) : opened.moves && calm ? (
          <Still url={url} name={file.name} width={size.width} height={size.height} />
        ) : (
          <img
            src={url}
            alt={file.name}
            draggable={false}
            style={{ aspectRatio: file.width > 0 && file.height > 0 ? `${size.width} / ${size.height}` : undefined }}
            className="w-full rounded-xl border bg-muted/60 object-contain"
          />
        )}
        <button
          type="button"
          onClick={() => save(opened.blob, file.name)}
          aria-label={`Download ${file.name}`}
          title={`Download ${file.name}`}
          className="absolute top-2 right-2 grid size-8 place-items-center rounded-lg bg-black/55 text-white opacity-0 shadow transition group-hover/sealed:opacity-100 hover:scale-110 focus-visible:opacity-100"
        >
          <DownloadIcon className="size-4" />
        </button>
      </motion.div>
    );
  }

  const size = opened ? opened.blob.size : file.fileSize;
  const busy = state.at === "opening";
  return (
    <motion.div
      ref={place}
      {...enterWith}
      className={cn("flex w-full max-w-sm items-center gap-3 rounded-xl border bg-muted/40 px-3 py-2.5", state.at === "failed" && "border-destructive/50")}
    >
      <span className="relative">
        <FileBadge name={file.name} />
        <span className="absolute -right-1 -bottom-1 grid size-4 place-items-center rounded-full bg-card text-emerald-500 shadow-sm" title="Encrypted">
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
            {state.at === "failed" ? (
              <span className="inline-flex items-center gap-1 first-letter:uppercase">
                <ShieldAlertIcon className="size-3" />
                {state.problem}
              </span>
            ) : busy ? (
              "Opening on this device…"
            ) : size > 0 ? (
              formatBytes(size)
            ) : (
              "Encrypted file"
            )}
          </motion.p>
        </AnimatePresence>
      </div>
      {hinted && !opened && (
        <CardButton label={`Show ${file.name}`} onClick={() => void open()} disabled={busy}>
          {busy ? <LoaderCircleIcon className="size-4 animate-spin" /> : <EyeIcon className="size-4" />}
        </CardButton>
      )}
      <CardButton label={`Download ${file.name}`} onClick={() => void download()} disabled={busy}>
        <DownloadIcon className="size-4" />
      </CardButton>
    </motion.div>
  );
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
 * it's clicked.
 */
function Still({ url, name, width, height }: { url: string; name: string; width: number; height: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [playing, setPlaying] = useState(false);
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
  if (playing) return <img src={url} alt={name} draggable={false} className="w-full rounded-xl border bg-muted/60 object-contain" />;
  return (
    <button
      type="button"
      onClick={() => setPlaying(true)}
      aria-label={`Play ${name}`}
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

/** Files on their way in an encrypted message: sealed and uploaded on this device before it's sent. */
export function PendingFiles({ files }: { files: { name: string; size: number }[] }) {
  return (
    <div className="mt-1 flex flex-col gap-1.5">
      {files.map((file, n) => (
        <div key={n} className="relative flex w-full max-w-sm items-center gap-3 overflow-hidden rounded-xl border bg-muted/40 px-3 py-2.5">
          <FileBadge name={file.name} />
          <div className="min-w-0 flex-1">
            <p className="truncate text-sm font-bold">{shortName(file.name, 48)}</p>
            <p className="truncate text-xs text-muted-foreground tabular-nums">{formatBytes(file.size)}</p>
          </div>
          <LockKeyholeIcon className="size-4 shrink-0 text-emerald-500" />
        </div>
      ))}
    </div>
  );
}
