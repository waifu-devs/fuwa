import * as DialogPrimitive from "@radix-ui/react-dialog";
import {
  ArchiveIcon,
  DownloadIcon,
  FileCodeIcon,
  FileIcon,
  FileImageIcon,
  FileMusicIcon,
  FileSpreadsheetIcon,
  FileTextIcon,
  FileVideoIcon,
  PresentationIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { memo, useMemo, useState, type ReactNode } from "react";
import type { Attachment } from "@/gen/fuwa/v1/types_pb";
import { familyOf, fitBox, lookOf, shortName, type FileFamily } from "@/lib/attachments";
import { formatBytes } from "@/lib/format";
import { useI18n } from "@/i18n/react";
import { usePrefs } from "@/lib/prefs";
import { shownPicture } from "@/lib/shown";
import { reportError } from "@/lib/reports";
import { cn } from "@/lib/utils";
import { VoiceMessage, VoiceProblem } from "@/components/voice/VoiceMessage";
import { attachmentLoader } from "@/voice/fetch";

/** The most room one picture or video takes in a message. */
const MEDIA_BOX = { width: 420, height: 320 };
/** Pictures side by side, when a message has more than one. */
const TILE_BOX = { width: 200, height: 200 };

const ICONS: Record<FileFamily, typeof FileIcon> = {
  archive: ArchiveIcon,
  code: FileCodeIcon,
  document: FileTextIcon,
  sheet: FileSpreadsheetIcon,
  slides: PresentationIcon,
  pdf: FileTextIcon,
  text: FileTextIcon,
  audio: FileMusicIcon,
  video: FileVideoIcon,
  picture: FileImageIcon,
  other: FileIcon,
};

/** Each family's tint, so a zip and a PDF are told apart at a glance. */
const TINTS: Record<FileFamily, string> = {
  archive: "text-amber-600 bg-amber-500/12 dark:text-amber-400",
  code: "text-sky-600 bg-sky-500/12 dark:text-sky-400",
  document: "text-blue-600 bg-blue-500/12 dark:text-blue-400",
  sheet: "text-emerald-600 bg-emerald-500/12 dark:text-emerald-400",
  slides: "text-orange-600 bg-orange-500/12 dark:text-orange-400",
  pdf: "text-rose-600 bg-rose-500/12 dark:text-rose-400",
  text: "text-muted-foreground bg-muted",
  audio: "text-violet-600 bg-violet-500/12 dark:text-violet-400",
  video: "text-fuchsia-600 bg-fuchsia-500/12 dark:text-fuchsia-400",
  picture: "text-pink-600 bg-pink-500/12 dark:text-pink-400",
  other: "text-primary bg-primary/12",
};

/** A file's icon in its family's tint. */
export function FileBadge({ name, className }: { name: string; className?: string }) {
  const family = familyOf(name);
  const Icon = ICONS[family];
  return (
    <span className={cn("grid size-10 shrink-0 place-items-center rounded-xl", TINTS[family], className)}>
      <Icon className="size-5" />
    </span>
  );
}

/**
 * Starts a download without showing its link: streamer mode keeps the
 * instance's address off screen, and a hovered link shows it in the corner.
 */
function downloadQuietly(url: string, name: string) {
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.rel = "noopener";
  document.body.append(a);
  a.click();
  a.remove();
}

/** A download button: a plain link normally, a button that hides it while streaming. */
function Download({ url, name, className, children }: { url: string; name: string; className?: string; children: ReactNode }) {
  const quiet = usePrefs((p) => p.streamer && p.streamerHidePersonal);
  if (quiet) {
    return (
      <button type="button" onClick={() => downloadQuietly(url, name)} aria-label={`Download ${name}`} className={className}>
        {children}
      </button>
    );
  }
  return (
    <a href={url} download={name} rel="noopener" aria-label={`Download ${name}`} className={className}>
      {children}
    </a>
  );
}

/**
 * The files a message came with: pictures shown (in a grid when there are
 * several), videos and audio with players, anything else as a card to
 * download. Only files on a fuwa instance load; each keeps its box before
 * it loads, so the list never jumps.
 */
export const Attachments = memo(function Attachments({ files, animate }: { files: Attachment[]; animate: boolean }) {
  const [open, setOpen] = useState<Attachment | null>(null);
  if (!files.length) return null;
  const pictures = files.filter((f) => lookOf(f.contentType) === "picture" && shownPicture(f.url));
  const shown = new Set(pictures);
  const rest = files.filter((f) => !shown.has(f));
  const tiled = pictures.length > 1;
  return (
    <div className="mt-1 flex flex-col gap-1.5">
      {pictures.length > 0 && (
        <div className={cn("flex flex-wrap gap-1.5", tiled && "max-w-[412px]")}>
          {pictures.map((file, n) => (
            <Picture key={file.id || file.url} file={file} box={tiled ? TILE_BOX : MEDIA_BOX} square={tiled} delay={animate ? n * 0.04 : 0} animate={animate} onOpen={() => setOpen(file)} />
          ))}
        </div>
      )}
      {rest.map((file, n) => {
        const look = lookOf(file.contentType);
        const delay = animate ? (pictures.length + n) * 0.04 : 0;
        if (look === "video" && shownPicture(file.url)) return <Video key={file.id || file.url} file={file} delay={delay} animate={animate} />;
        if (look === "audio" && file.voice && shownPicture(file.url)) return <Voice key={file.id || file.url} file={file} delay={delay} animate={animate} />;
        if (look === "audio" && shownPicture(file.url)) return <Audio key={file.id || file.url} file={file} delay={delay} animate={animate} />;
        return <FileCard key={file.id || file.url} file={file} delay={delay} animate={animate} />;
      })}
      <Viewer file={open} onClose={() => setOpen(null)} />
    </div>
  );
});

function enter(animate: boolean, delay: number) {
  return animate
    ? { initial: { opacity: 0, y: 6, scale: 0.97 }, animate: { opacity: 1, y: 0, scale: 1 }, transition: { type: "spring", stiffness: 460, damping: 30, delay } as const }
    : {};
}

function Picture({
  file,
  box,
  square,
  delay,
  animate,
  onOpen,
}: {
  file: Attachment;
  box: { width: number; height: number };
  square: boolean;
  delay: number;
  animate: boolean;
  onOpen: () => void;
}) {
  const [loaded, setLoaded] = useState(false);
  const [broken, setBroken] = useState(false);
  const size = square ? box : fitBox(file.width, file.height, box);
  if (broken) return <FileCard file={file} delay={0} animate={false} />;
  return (
    <motion.button
      type="button"
      {...enter(animate, delay)}
      whileHover={{ scale: 1.01 }}
      whileTap={{ scale: 0.98 }}
      onClick={onOpen}
      aria-label={`Open ${file.filename}`}
      style={{ width: size.width, aspectRatio: `${size.width} / ${size.height}` }}
      className="relative max-w-full overflow-hidden rounded-xl border bg-muted/60"
    >
      {!loaded && <span aria-hidden className="absolute inset-0 animate-pulse bg-muted" />}
      <img
        src={shownPicture(file.url)}
        alt={file.filename}
        loading="lazy"
        decoding="async"
        draggable={false}
        onLoad={() => setLoaded(true)}
        onError={() => {
          setBroken(true);
          reportError("attachment_unavailable", "picture");
        }}
        className={cn("size-full transition-opacity duration-300", square ? "object-cover" : "object-contain", loaded ? "opacity-100" : "opacity-0")}
      />
    </motion.button>
  );
}

function Video({ file, delay, animate }: { file: Attachment; delay: number; animate: boolean }) {
  const [broken, setBroken] = useState(false);
  const size = fitBox(file.width, file.height, MEDIA_BOX);
  if (broken) return <FileCard file={file} delay={0} animate={false} />;
  return (
    <motion.div {...enter(animate, delay)} className="flex max-w-full flex-col gap-1" style={{ width: size.width }}>
      <video
        src={shownPicture(file.url)}
        controls
        playsInline
        preload="metadata"
        onError={() => {
          setBroken(true);
          reportError("attachment_unavailable", "video");
        }}
        style={{ aspectRatio: `${size.width} / ${size.height}` }}
        className="w-full rounded-xl border bg-black"
      />
      <FileLine file={file} />
    </motion.div>
  );
}

function Audio({ file, delay, animate }: { file: Attachment; delay: number; animate: boolean }) {
  const [broken, setBroken] = useState(false);
  if (broken) return <FileCard file={file} delay={0} animate={false} />;
  return (
    <motion.div {...enter(animate, delay)} className="flex w-full max-w-md flex-col gap-2 rounded-2xl border bg-card/70 p-3 shadow-sm">
      <div className="flex items-center gap-3">
        <FileBadge name={file.filename || "audio.mp3"} />
        <FileLine file={file} />
      </div>
      <audio
        src={shownPicture(file.url)}
        controls
        preload="none"
        onError={() => {
          setBroken(true);
          reportError("attachment_unavailable", "audio");
        }}
        className="h-10 w-full"
      />
    </motion.div>
  );
}

/**
 * A voice message recorded in the app: its waveform drawn as bars from what
 * the sender's app sent, the sound fetched only when it's first played,
 * from a fuwa instance and no bigger than the file is.
 */
function Voice({ file, delay, animate }: { file: Attachment; delay: number; animate: boolean }) {
  const voice = file.voice!;
  const load = useMemo(() => attachmentLoader(file.url, Number(file.size)), [file.url, file.size]);
  const id = `attachment:${file.id || file.url}`;
  return (
    <motion.div {...enter(animate, delay)} className="w-full max-w-[22rem]">
      <VoiceMessage id={id} durationMs={voice.durationMs} waveform={voice.waveform} load={load} />
      <VoiceProblem id={id} />
    </motion.div>
  );
}

/** A file's name and size, with its download button. */
function FileLine({ file }: { file: Attachment }) {
  const lang = useI18n();
  return (
    <div className="flex min-w-0 flex-1 items-center gap-2">
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-bold" title={file.filename}>
          {shortName(file.filename)}
        </p>
        <p className="text-xs text-muted-foreground tabular-nums">{formatBytes(lang, Number(file.size))}</p>
      </div>
      {shownPicture(file.url) && (
        <Download url={file.url} name={file.filename} className="grid size-8 shrink-0 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-muted hover:text-primary">
          <DownloadIcon className="size-4" />
        </Download>
      )}
    </div>
  );
}

function FileCard({ file, delay, animate }: { file: Attachment; delay: number; animate: boolean }) {
  return (
    <motion.div
      {...enter(animate, delay)}
      whileHover={{ y: -1 }}
      className="group/file flex w-full max-w-md items-center gap-3 rounded-2xl border bg-card/70 p-3 shadow-sm"
    >
      <motion.span whileHover={{ rotate: -8, scale: 1.06 }} transition={{ type: "spring", stiffness: 500, damping: 18 }}>
        <FileBadge name={file.filename} />
      </motion.span>
      <FileLine file={file} />
    </motion.div>
  );
}

/** A picture full size over everything, closed with Escape, a click outside or the button. */
function Viewer({ file, onClose }: { file: Attachment | null; onClose: () => void }) {
  const lang = useI18n();
  return (
    <DialogPrimitive.Root open={!!file} onOpenChange={(open) => !open && onClose()}>
      <AnimatePresence>
        {file && (
          <DialogPrimitive.Portal forceMount>
            <DialogPrimitive.Overlay asChild forceMount>
              <motion.div
                className="fixed inset-0 z-50 bg-black/80 backdrop-blur-sm"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.18 }}
              />
            </DialogPrimitive.Overlay>
            <DialogPrimitive.Content asChild forceMount aria-describedby={undefined}>
              <motion.div
                className="fixed inset-0 z-50 flex flex-col items-center justify-center gap-3 p-4 outline-none"
                onClick={(e) => e.target === e.currentTarget && onClose()}
              >
                <DialogPrimitive.Title className="sr-only">{file.filename}</DialogPrimitive.Title>
                <motion.img
                  src={shownPicture(file.url)}
                  alt={file.filename}
                  initial={{ opacity: 0, scale: 0.92 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.95 }}
                  transition={{ type: "spring", stiffness: 420, damping: 32 }}
                  className="max-h-[82svh] max-w-full rounded-xl object-contain shadow-2xl"
                />
                <motion.div
                  initial={{ opacity: 0, y: 8 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: 8 }}
                  transition={{ type: "spring", stiffness: 420, damping: 32, delay: 0.05 }}
                  className="flex max-w-full items-center gap-2 rounded-full bg-black/60 py-1.5 pr-1.5 pl-4 text-sm text-white"
                >
                  <span className="truncate font-bold" title={file.filename}>
                    {shortName(file.filename, 48)}
                  </span>
                  <span className="shrink-0 text-white/70 tabular-nums">{formatBytes(lang, Number(file.size))}</span>
                  <Download url={file.url} name={file.filename} className="grid size-8 shrink-0 place-items-center rounded-full transition-colors hover:bg-white/15">
                    <DownloadIcon className="size-4" />
                  </Download>
                  <DialogPrimitive.Close className="grid size-8 shrink-0 place-items-center rounded-full transition hover:rotate-90 hover:bg-white/15">
                    <XIcon className="size-4" />
                    <span className="sr-only">Close</span>
                  </DialogPrimitive.Close>
                </motion.div>
              </motion.div>
            </DialogPrimitive.Content>
          </DialogPrimitive.Portal>
        )}
      </AnimatePresence>
    </DialogPrimitive.Root>
  );
}

/** Files on their way with a message: named, not loaded (they're served once it's sent). */
export function PendingFiles({ files }: { files: Attachment[] }) {
  if (!files.length) return null;
  return (
    <div className="mt-1 flex flex-wrap gap-1.5">
      {files.map((file) =>
        file.voice ? (
          <VoiceMessage key={file.url} id={`pending:${file.url}`} durationMs={file.voice.durationMs} waveform={file.voice.waveform} load={null} pending />
        ) : (
        <span key={file.url} className="flex max-w-60 items-center gap-2 rounded-xl border bg-card/70 py-1.5 pr-3 pl-1.5 text-xs">
          <FileBadge name={file.filename} className="size-7 rounded-lg [&_svg]:size-4" />
          <span className="truncate font-bold">{shortName(file.filename, 28)}</span>
        </span>
        ),
      )}
    </div>
  );
}
