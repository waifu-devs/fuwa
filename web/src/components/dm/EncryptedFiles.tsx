import { PaperclipIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useRef, useSyncExternalStore } from "react";
import { FileBadge } from "@/components/chat/Attachments";
import { SPRING } from "@/components/motion";
import { cleanName, MAX_FILES } from "@/files/sealed";
import { cantSendFiles } from "@/fuwa/dms";
import { shortName } from "@/lib/attachments";
import { formatBytes } from "@/lib/format";
import { toast } from "@/lib/ui";

/**
 * Files picked for the next encrypted message, by conversation (or thread).
 * Nothing leaves this device until the message is sent: then each is sealed
 * here and uploaded as ciphertext.
 */
const NONE: File[] = [];
const picked = new Map<string, File[]>();
const listeners = new Set<() => void>();

function set(draft: string, files: File[]) {
  if (files.length) picked.set(draft, files);
  else picked.delete(draft);
  for (const l of listeners) l();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export function usePicked(draft: string): File[] {
  return useSyncExternalStore(subscribe, () => picked.get(draft) ?? NONE);
}

/** Adds files to the next message, up to what one can carry. */
export function pickFiles(draft: string, files: File[]) {
  if (!files.length) return;
  const next = [...(picked.get(draft) ?? NONE), ...files];
  const problem = cantSendFiles(next);
  if (problem) toast(problem);
  set(draft, next.slice(0, MAX_FILES).filter((f) => !cantSendFiles([f])));
}

export const clearPicked = (draft: string) => set(draft, []);

/** The paperclip in an encrypted composer. */
export function EncryptedAttach({ draft, disabled }: { draft: string; disabled?: boolean }) {
  const input = useRef<HTMLInputElement>(null);
  return (
    <>
      <motion.button
        type="button"
        aria-label="Attach files"
        title="Attach files (encrypted on this device)"
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
          pickFiles(draft, Array.from(e.target.files ?? []));
          e.target.value = "";
        }}
      />
    </>
  );
}

/** The files going with the next encrypted message. */
export function PickedTray({ draft, files }: { draft: string; files: File[] }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: 8 }}
      transition={SPRING}
      className="scroll-thin -mx-1 mb-1.5 flex gap-2 overflow-x-auto px-1 pt-1 pb-1"
      role="list"
      aria-label="Files to send"
    >
      <AnimatePresence initial={false} mode="popLayout">
        {files.map((file, n) => {
          const name = cleanName(file.name);
          return (
            <motion.div
              key={`${n}:${file.name}:${file.size}:${file.lastModified}`}
              layout
              role="listitem"
              initial={{ opacity: 0, scale: 0.8, y: 10 }}
              animate={{ opacity: 1, scale: 1, y: 0 }}
              exit={{ opacity: 0, scale: 0.8, transition: { duration: 0.15 } }}
              transition={SPRING}
              className="relative flex h-20 w-36 shrink-0 flex-col overflow-hidden rounded-xl border bg-muted/40"
              title={`${name} · ${formatBytes(file.size)}`}
            >
              <div className="flex flex-1 items-center justify-center pt-1">
                <FileBadge name={name} />
              </div>
              <div className="px-2 pb-1.5">
                <p className="truncate text-[0.7rem] font-bold">{shortName(name, 22)}</p>
                <p className="truncate text-[0.65rem] text-muted-foreground tabular-nums">{formatBytes(file.size)}</p>
              </div>
              <button
                type="button"
                aria-label={`Remove ${name}`}
                onClick={() => set(draft, files.filter((_, i) => i !== n))}
                className="absolute top-1 right-1 grid size-6 place-items-center rounded-full bg-card/90 text-foreground shadow-sm transition hover:rotate-90 hover:text-destructive"
              >
                <XIcon className="size-3.5" />
              </button>
            </motion.div>
          );
        })}
      </AnimatePresence>
    </motion.div>
  );
}
