import { EyeOffIcon, PaperclipIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useRef } from "react";
import { FileBadge } from "@/components/chat/Attachments";
import { SpoilerBadge } from "@/components/chat/SpoilerCover";
import { SPRING } from "@/lib/motion";
import { cleanName } from "@/files/sealed";
import { shortName } from "@/lib/attachments";
import { formatBytes } from "@/lib/format";
import { useI18n } from "@/i18n/react";
import { isSpoiler, pickFiles, setPicked, toggleSpoiler } from "@/components/dm/use-picked";
import { cn } from "@/lib/utils";

/** The paperclip in an encrypted composer. */
export function EncryptedAttach({ draft, disabled }: { draft: string; disabled?: boolean }) {
  const input = useRef<HTMLInputElement>(null);
  const { t } = useI18n();
  return (
    <>
      <motion.button
        type="button"
        aria-label={t("dms-calls.dm.files.attach")}
        title={t("dms-calls.dm.files.attachTitle")}
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

/** The files going with the next encrypted message, each of which can be marked to go as a spoiler. */
export function PickedTray({ draft, files }: { draft: string; files: File[] }) {
  const lang = useI18n();
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: 8 }}
      transition={SPRING}
      className="scroll-thin -mx-1 mb-1.5 flex gap-2 overflow-x-auto px-1 pt-1 pb-1"
      role="list"
      aria-label={lang.t("dms-calls.dm.files.tray")}
    >
      <AnimatePresence initial={false} mode="popLayout">
        {files.map((file, n) => {
          const name = cleanName(file.name);
          const spoiler = isSpoiler(file);
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
              title={`${name} · ${formatBytes(lang, file.size)}`}
            >
              <div className="flex flex-1 items-center justify-center pt-1">
                <FileBadge name={name} className={cn("transition-[filter] duration-300 motion-reduce:transition-none", spoiler && "blur-[6px]")} />
              </div>
              {/* Marked as a spoiler: covered here as it will be for whoever reads it. */}
              <AnimatePresence>
                {spoiler && (
                  <motion.span
                    key="spoiler"
                    initial={{ opacity: 0, scale: 0.6 }}
                    animate={{ opacity: 1, scale: 1 }}
                    exit={{ opacity: 0, scale: 0.6 }}
                    transition={SPRING}
                    className="pointer-events-none absolute inset-x-0 top-5 flex justify-center"
                  >
                    <SpoilerBadge className="px-2 py-0.5 text-[0.6rem]" />
                  </motion.span>
                )}
              </AnimatePresence>
              <div className="px-2 pb-1.5">
                <p className="truncate text-[0.7rem] font-bold">{shortName(name, 22)}</p>
                <p className="truncate text-[0.65rem] text-muted-foreground tabular-nums">{formatBytes(lang, file.size)}</p>
              </div>
              <div className="absolute top-1 right-1 flex gap-1">
                <motion.button
                  type="button"
                  aria-label={lang.t(spoiler ? "chat.files.unspoiler" : "chat.files.spoiler", { name })}
                  title={lang.t(spoiler ? "chat.files.unspoiler" : "chat.files.spoiler", { name })}
                  aria-pressed={spoiler}
                  onClick={() => toggleSpoiler(draft, file)}
                  whileTap={{ scale: 0.85 }}
                  className={cn(
                    "grid size-6 place-items-center rounded-full shadow-sm transition-colors",
                    spoiler ? "bg-primary text-primary-foreground" : "bg-card/90 text-foreground hover:text-primary",
                  )}
                >
                  <EyeOffIcon className="size-3.5" />
                </motion.button>
                <button
                  type="button"
                  aria-label={lang.t("dms-calls.dm.files.remove", { name })}
                  onClick={() => setPicked(draft, files.filter((_, i) => i !== n))}
                  className="grid size-6 place-items-center rounded-full bg-card/90 text-foreground shadow-sm transition hover:rotate-90 hover:text-destructive"
                >
                  <XIcon className="size-3.5" />
                </button>
              </div>
            </motion.div>
          );
        })}
      </AnimatePresence>
    </motion.div>
  );
}
