import { XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";
import type { Channel, SharedServer } from "@/gen/fuwa/v1/types_pb";
import { ServerIcon } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { sharedLabel } from "@/lib/shared";
import { cn } from "@/lib/utils";

/*
 * The marks a shared channel carries wherever it shows: a badge in the
 * sidebar, a pill in the header, a note at its start, and a tag beside the
 * names of people from the other server.
 */

/** Two linked rings: the app's sign for a channel shared between servers. */
export function SharedGlyph({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden className={cn("size-3.5", className)}>
      <circle cx="5.75" cy="8" r="3.75" />
      <circle cx="10.25" cy="8" r="3.75" />
    </svg>
  );
}

/** The sidebar's mark beside a shared channel's name: pops in, and says who it's shared with. */
export function SharedBadge({ channel, className }: { channel: Pick<Channel, "shared">; className?: string }) {
  const label = sharedLabel(channel);
  return (
    <AnimatePresence initial={false}>
      {label && (
        <motion.span
          key="shared"
          initial={{ scale: 0, rotate: -40, opacity: 0 }}
          animate={{ scale: 1, rotate: 0, opacity: 1 }}
          exit={{ scale: 0, rotate: 40, opacity: 0 }}
          transition={{ type: "spring", stiffness: 600, damping: 18 }}
          className={cn("inline-grid shrink-0 place-items-center", className)}
        >
          <Tooltip>
            <TooltipTrigger asChild>
              <span
                role="img"
                aria-label={label.text}
                data-arrange-skip
                className="grid size-5 place-items-center rounded-full text-primary/80 transition hover:scale-110 hover:bg-primary/10 hover:text-primary"
              >
                <SharedGlyph />
              </span>
            </TooltipTrigger>
            <TooltipContent side="right" className="font-bold">
              {label.text}
            </TooltipContent>
          </Tooltip>
        </motion.span>
      )}
    </AnimatePresence>
  );
}

/** The header's pill: where a shared channel's messages live, or who it's shown to. */
export function SharedPill({ channel }: { channel: Pick<Channel, "shared"> }) {
  const label = sharedLabel(channel);
  const text = !label ? "" : label.home ? label.text : `Shared · messages live on ${label.names || "another server"}`;
  return (
    <AnimatePresence initial={false}>
      {label && (
        <motion.span
          key="shared-pill"
          layout="position"
          initial={{ opacity: 0, scale: 0.7, x: -6 }}
          animate={{ opacity: 1, scale: 1, x: 0 }}
          exit={{ opacity: 0, scale: 0.7 }}
          transition={SPRING}
          title={text}
          className="flex min-w-0 shrink items-center gap-1.5 rounded-full bg-primary/10 px-2 py-0.5 text-xs font-bold text-primary"
        >
          <SharedGlyph className="shrink-0" />
          {/* On phones the glyph alone; the title says the rest. */}
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.span
              key={text}
              initial={{ opacity: 0, y: "0.6em" }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: "-0.6em" }}
              transition={SPRING}
              className="hidden truncate sm:inline"
            >
              {text}
            </motion.span>
          </AnimatePresence>
        </motion.span>
      )}
    </AnimatePresence>
  );
}

/** Beside the name of someone from another server: which one, as a small muted chip. */
export function ServerTag({ server, className }: { server: SharedServer; className?: string }) {
  return (
    <span
      title={server.instance ? `From ${server.name} on ${server.instance}` : `From ${server.name}`}
      className={cn(
        "server-tag inline-flex shrink-0 items-center gap-1 rounded-full bg-muted px-1.5 py-px align-middle text-[0.68rem] leading-4 font-bold text-muted-foreground",
        server.instance ? "max-w-64 min-w-0 shrink" : "max-w-40",
        className,
      )}
    >
      <ServerIcon server={server} className="size-3.5 rounded-full text-[0.45rem]" />
      <span className={cn("truncate", server.instance && "max-w-28 shrink-0")}>{server.name}</span>
      {server.instance && <span className="min-w-0 truncate font-normal opacity-80">· {server.instance}</span>}
    </span>
  );
}

const NOTE_KEY = (channelId: string) => `fuwa.shared-note.${channelId}`;

function noteDismissed(channelId: string) {
  try {
    return localStorage.getItem(NOTE_KEY(channelId)) === "1";
  } catch {
    return false;
  }
}

/**
 * Said once at the start of a shared channel: which servers talk here and
 * where what's said is kept. Closing it is remembered in this browser.
 */
export function SharedNote({ channel }: { channel: Pick<Channel, "id" | "shared"> }) {
  const [hidden, setHidden] = useState(() => noteDismissed(channel.id));
  const label = sharedLabel(channel);
  const shared = channel.shared;
  if (!label || !shared) return null;
  const home = shared.home ? "this server" : (shared.homeServer?.name ?? "the other server");
  const others = label.names || "another server";
  return (
    <AnimatePresence initial={false}>
      {!hidden && (
        <motion.div
          key="note"
          initial={{ opacity: 0, y: 10, scale: 0.98 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: -6, scale: 0.98, transition: { duration: 0.18 } }}
          transition={{ ...SPRING, delay: 0.15 }}
          className="relative mt-4 flex max-w-xl items-start gap-3 rounded-2xl border border-primary/25 bg-primary/5 p-3 pr-10"
        >
          <motion.span
            initial={{ rotate: -90, scale: 0.4 }}
            animate={{ rotate: 0, scale: 1 }}
            transition={{ type: "spring", stiffness: 380, damping: 14, delay: 0.3 }}
            className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary/15 text-primary"
          >
            <SharedGlyph className="size-5" />
          </motion.span>
          <div className="min-w-0 text-sm">
            <p className="font-bold">{shared.home ? `You share this channel with ${others}` : `This channel comes from ${others}`}</p>
            <p className="text-muted-foreground">
              People from both servers read and write here. Messages are kept only on {home}, and each server looks after its own people.
            </p>
          </div>
          <button
            type="button"
            aria-label="Got it"
            title="Got it"
            onClick={() => {
              setHidden(true);
              try {
                localStorage.setItem(NOTE_KEY(channel.id), "1");
              } catch {
                // Private windows may not keep it; the note just comes back.
              }
            }}
            className="absolute top-2 right-2 grid size-7 place-items-center rounded-full text-muted-foreground transition hover:rotate-90 hover:bg-muted hover:text-foreground"
          >
            <XIcon className="size-4" />
          </button>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
