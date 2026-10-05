import { CheckIcon, PaperclipIcon, RotateCwIcon, UploadIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { FileBadge } from "@/components/chat/Attachments";
import { addFiles, removeFile, retryFile, type Staged } from "@/components/chat/staged";
import { SPRING } from "@/components/motion";
import { shortName } from "@/lib/attachments";
import { formatBytes } from "@/lib/format";
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

type Where = { instanceKey: string; serverId: string; channelId: string };

/** The paperclip: picks files from the device for the next message. */
export function AttachButton({ where, disabled }: { where: Where; disabled?: boolean }) {
  const { t } = useI18n();
  const input = useRef<HTMLInputElement>(null);
  return (
    <>
      <motion.button
        type="button"
        aria-label={t("chat.files.attach")}
        title={t("chat.files.attach")}
        disabled={disabled}
        onClick={() => input.current?.click()}
        whileHover={{ scale: 1.12, rotate: 12 }}
        whileTap={{ scale: 0.85 }}
        className="mb-0.5 grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition-colors hover:text-primary disabled:opacity-40"
      >
        <PaperclipIcon className="size-[18px]" />
      </motion.button>
      <input
        ref={input}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          addFiles(where.instanceKey, where.serverId, where.channelId, Array.from(e.target.files ?? []));
          e.target.value = "";
        }}
      />
    </>
  );
}

/** The files going with the next message, each with how far its upload got. */
export function StagedTray({ where, staged }: { where: Where; staged: Staged[] }) {
  const { t } = useI18n();
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: 8 }}
      transition={SPRING}
      className="scroll-thin -mx-1 flex gap-2 overflow-x-auto px-1 pt-1 pb-2"
      role="list"
      aria-label={t("chat.files.toSend")}
    >
      <AnimatePresence initial={false} mode="popLayout">
        {staged.map((s) => (
          <StagedCard key={s.id} where={where} staged={s} />
        ))}
      </AnimatePresence>
    </motion.div>
  );
}

function StagedCard({ where, staged }: { where: Where; staged: Staged }) {
  const lang = useI18n();
  const { t } = lang;
  const { file, preview, sent, done, failed } = staged;
  return (
    <motion.div
      layout
      role="listitem"
      initial={{ opacity: 0, scale: 0.8, y: 10 }}
      animate={{ opacity: 1, scale: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.8, transition: { duration: 0.15 } }}
      transition={SPRING}
      className={cn(
        "group/staged relative flex h-24 w-36 shrink-0 flex-col overflow-hidden rounded-xl border bg-muted/40",
        failed && "border-destructive/60 bg-destructive/5",
      )}
      title={failed ?? t("chat.files.nameAndSize", { name: file.name, size: formatBytes(lang, file.size) })}
    >
      {preview ? (
        <img src={preview} alt="" className={cn("absolute inset-0 size-full object-cover transition-opacity duration-300", !done && "opacity-60")} />
      ) : (
        <div className="flex flex-1 items-center justify-center pt-1">
          <FileBadge name={file.name} />
        </div>
      )}
      <div className={cn("relative mt-auto px-2 pb-1.5", preview && "bg-gradient-to-t from-black/70 to-transparent pt-4 text-white")}>
        <p className="truncate text-[0.7rem] font-bold">{shortName(file.name, 22)}</p>
        <p className={cn("truncate text-[0.65rem] tabular-nums", preview ? "text-white/75" : "text-muted-foreground", failed && "text-destructive")}>
          {failed ?? formatBytes(lang, file.size)}
        </p>
      </div>
      {/* How far it got: a bar along the bottom that fills, then fades once done. */}
      <motion.span
        aria-hidden
        className="absolute bottom-0 left-0 h-1 w-full origin-left bg-primary"
        initial={false}
        animate={{ scaleX: failed ? 0 : sent, opacity: done || failed ? 0 : 1 }}
        transition={{ scaleX: { type: "spring", stiffness: 200, damping: 30 }, opacity: { delay: done ? 0.4 : 0, duration: 0.3 } }}
      />
      <AnimatePresence>
        {done && (
          <motion.span
            key="done"
            initial={{ scale: 0, opacity: 0 }}
            animate={{ scale: [0, 1.25, 1, 1], opacity: [0, 1, 1, 0] }}
            transition={{ duration: 1.1, times: [0, 0.25, 0.4, 1] }}
            className="absolute top-1.5 left-1.5 grid size-5 place-items-center rounded-full bg-primary text-primary-foreground"
          >
            <CheckIcon className="size-3" />
          </motion.span>
        )}
      </AnimatePresence>
      <div className="absolute top-1 right-1 flex gap-1">
        {failed && (
          <button
            type="button"
            aria-label={t("chat.files.retry", { name: file.name })}
            onClick={() => retryFile(where.instanceKey, where.serverId, where.channelId, staged.id)}
            className="grid size-6 place-items-center rounded-full bg-card/90 text-foreground shadow-sm transition hover:rotate-45 hover:text-primary"
          >
            <RotateCwIcon className="size-3.5" />
          </button>
        )}
        <button
          type="button"
          aria-label={t("chat.files.remove", { name: file.name })}
          onClick={() => removeFile(where.channelId, staged.id)}
          className="grid size-6 place-items-center rounded-full bg-card/90 text-foreground shadow-sm transition hover:rotate-90 hover:text-destructive"
        >
          <XIcon className="size-3.5" />
        </button>
      </div>
    </motion.div>
  );
}

/**
 * While files are dragged over the window: a sheet saying where they'll go.
 * Dropping them anywhere adds them to the next message. Off while a dialog
 * is open, which may take files of its own (a picture, a theme).
 */
export function DropOverlay({
  where,
  channelName,
  onFiles,
  note,
}: {
  where?: Where;
  /** The channel they'll go to, or (starting with "@") the person. */
  channelName: string;
  /** Takes the dropped files instead of staging them for a channel. */
  onFiles?: (files: File[]) => void;
  note?: string;
}) {
  const { t } = useI18n();
  const [over, setOver] = useState(false);
  useEffect(() => {
    let depth = 0;
    const files = (e: DragEvent) => !!e.dataTransfer?.types.includes("Files") && !document.querySelector("[role=dialog]");
    const enter = (e: DragEvent) => {
      if (!files(e)) return;
      depth++;
      setOver(true);
    };
    const leave = (e: DragEvent) => {
      if (!files(e)) return;
      depth = Math.max(0, depth - 1);
      if (!depth) setOver(false);
    };
    const hover = (e: DragEvent) => {
      if (!files(e)) return;
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
    };
    const drop = (e: DragEvent) => {
      depth = 0;
      setOver(false);
      if (e.defaultPrevented || !files(e)) return;
      e.preventDefault();
      const dropped = Array.from(e.dataTransfer?.files ?? []);
      if (onFiles) onFiles(dropped);
      else if (where) addFiles(where.instanceKey, where.serverId, where.channelId, dropped);
    };
    window.addEventListener("dragenter", enter);
    window.addEventListener("dragleave", leave);
    window.addEventListener("dragover", hover);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragenter", enter);
      window.removeEventListener("dragleave", leave);
      window.removeEventListener("dragover", hover);
      window.removeEventListener("drop", drop);
    };
  }, [where?.instanceKey, where?.serverId, where?.channelId, onFiles]);
  return (
    <AnimatePresence>
      {over && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.15 }}
          className="pointer-events-none fixed inset-0 z-40 grid place-items-center bg-background/60 p-6 backdrop-blur-[2px]"
        >
          <motion.div
            initial={{ scale: 0.9, y: 12 }}
            animate={{ scale: 1, y: 0 }}
            exit={{ scale: 0.95, y: 6 }}
            transition={SPRING}
            className="flex max-w-sm flex-col items-center gap-3 rounded-3xl border-2 border-dashed border-primary/60 bg-card px-10 py-8 text-center shadow-2xl"
          >
            <motion.span
              animate={{ y: [0, -6, 0] }}
              transition={{ duration: 1.2, repeat: Infinity, ease: "easeInOut" }}
              className="grid size-14 place-items-center rounded-2xl bg-primary/15 text-primary"
            >
              <UploadIcon className="size-7" />
            </motion.span>
            <p className="text-lg font-extrabold">
              {channelName.startsWith("@") ? t("chat.files.dropToPerson", { name: channelName }) : t("chat.files.dropToChannel", { channel: channelName })}
            </p>
            <p className="text-sm text-muted-foreground">{note ?? t("chat.files.dropNote")}</p>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/** Around the send button while files are still going up: how far they've got, together. */
export function UploadRing({ share }: { share: number }) {
  const r = 15;
  const length = 2 * Math.PI * r;
  return (
    <motion.span
      initial={{ scale: 0.4, opacity: 0 }}
      animate={{ scale: 1, opacity: 1 }}
      exit={{ scale: 1.4, opacity: 0 }}
      transition={{ type: "spring", stiffness: 600, damping: 20 }}
      className="relative grid size-9 place-items-center text-primary"
    >
      <svg viewBox="0 0 36 36" className="absolute inset-0 -rotate-90" aria-hidden>
        <circle cx="18" cy="18" r={r} fill="none" strokeWidth="2.5" className="stroke-primary/15" />
        <circle
          cx="18"
          cy="18"
          r={r}
          fill="none"
          strokeWidth="2.5"
          strokeLinecap="round"
          strokeDasharray={length}
          strokeDashoffset={length * (1 - share)}
          className="stroke-primary transition-[stroke-dashoffset] duration-300 ease-out"
        />
      </svg>
      <UploadIcon className="relative size-3.5" />
    </motion.span>
  );
}
