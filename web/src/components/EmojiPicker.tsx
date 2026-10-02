import {
  autoUpdate,
  flip,
  FloatingFocusManager,
  FloatingPortal,
  offset,
  shift,
  useClick,
  useDismiss,
  useFloating,
  useInteractions,
  useRole,
  type Placement,
} from "@floating-ui/react";
import { SearchIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useMemo, useState, type ReactElement, type ReactNode } from "react";
import type { Emoji } from "@/gen/fuwa/v1/types_pb";
import { ServerIcon } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { emojiToken, UNICODE_EMOJI } from "@/lib/emoji";
import { cn } from "@/lib/utils";

/** What a pick gives: the text to put in (a Unicode emoji, or `<:name:id>`), and a name to show. */
export type PickedEmoji = { text: string; name: string };

/**
 * A grid of emoji that opens from `trigger`: the server's own first, then
 * everyday ones, with a search box. Each pops a little on hover.
 */
export function EmojiPicker({
  emojis,
  server,
  onPick,
  children,
  placement = "top-end",
  closeOnPick = true,
}: {
  emojis: Emoji[] | undefined;
  /** The server the emoji belong to, for the section's icon. */
  server?: { id: string; name: string; iconUrl: string };
  onPick: (emoji: PickedEmoji) => void;
  /** The button that opens it. */
  children: (open: boolean) => ReactElement;
  placement?: Placement;
  closeOnPick?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [hovered, setHovered] = useState<PickedEmoji | null>(null);
  const { refs, floatingStyles, context, elements } = useFloating({
    open,
    onOpenChange: (o) => {
      setOpen(o);
      if (!o) setQuery("");
    },
    placement,
    whileElementsMounted: autoUpdate,
    middleware: [offset(8), flip(), shift({ padding: 8 })],
  });
  const { getReferenceProps, getFloatingProps } = useInteractions([useClick(context), useDismiss(context), useRole(context, { role: "dialog" })]);

  // Inside a dialog it opens in the dialog, which keeps pointer events (and clicks) to itself.
  const dialog = (elements.domReference as Element | null)?.closest<HTMLElement>("[role=dialog]") ?? null;
  const q = query.trim().toLowerCase().replace(/^:|:$/g, "");
  const own = useMemo(() => (emojis ?? []).filter((e) => e.name.toLowerCase().includes(q)).sort((a, b) => a.name.localeCompare(b.name)), [emojis, q]);
  const plain = useMemo(() => UNICODE_EMOJI.filter((e) => !q || e.names.some((n) => n.includes(q))), [q]);

  function pick(emoji: PickedEmoji) {
    onPick(emoji);
    if (closeOnPick) {
      setOpen(false);
      setQuery("");
    }
  }

  const cell = (key: string, picked: PickedEmoji, body: ReactNode, n: number) => (
    <motion.button
      key={key}
      type="button"
      initial={{ opacity: 0, scale: 0.6 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{ ...SPRING, delay: Math.min(n, 24) * 0.008 }}
      whileHover={{ scale: 1.3, rotate: -6 }}
      whileTap={{ scale: 0.85 }}
      onMouseEnter={() => setHovered(picked)}
      onFocus={() => setHovered(picked)}
      onClick={() => pick(picked)}
      aria-label={`:${picked.name}:`}
      className="grid size-9 place-items-center rounded-lg text-2xl leading-none hover:bg-primary/10 focus-visible:bg-primary/10 focus-visible:outline-none"
    >
      {body}
    </motion.button>
  );

  return (
    <>
      {(() => {
        const trigger = children(open);
        return <span ref={refs.setReference} {...getReferenceProps()} className="inline-flex">{trigger}</span>;
      })()}
      <FloatingPortal key={dialog ? "dialog" : "body"} root={dialog ?? undefined}>
        <AnimatePresence>
          {open && (
            <FloatingFocusManager context={context} initialFocus={0} modal={false}>
              <div ref={refs.setFloating} style={floatingStyles} className="z-50" {...getFloatingProps()}>
                <motion.div
                  initial={{ opacity: 0, scale: 0.92, y: 8 }}
                  animate={{ opacity: 1, scale: 1, y: 0 }}
                  exit={{ opacity: 0, scale: 0.95, y: 6 }}
                  transition={SPRING}
                  className="flex w-[19rem] max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-2xl border bg-popover shadow-2xl"
                >
                  <div className="relative border-b p-2">
                    <SearchIcon className="pointer-events-none absolute top-1/2 left-4.5 size-4 -translate-y-1/2 text-muted-foreground" />
                    <input
                      value={query}
                      onChange={(e) => setQuery(e.target.value)}
                      placeholder="Find an emoji"
                      aria-label="Find an emoji"
                      className="h-9 w-full rounded-xl bg-muted/60 pr-3 pl-8 text-sm outline-none focus:ring-2 focus:ring-primary/40"
                    />
                  </div>
                  <div className="scroll-thin h-64 overflow-y-auto p-2">
                    {own.length > 0 && (
                      <section className="mb-2">
                        <p className="sticky top-0 z-10 flex items-center gap-1.5 bg-popover/95 px-1 pb-1 text-[0.65rem] font-extrabold tracking-wide text-muted-foreground uppercase backdrop-blur">
                          {server && <ServerIcon server={server} className="size-4 rounded-md text-[0.4rem]" />}
                          {server?.name ?? "This server"}
                        </p>
                        <div className="grid grid-cols-7 gap-0.5">
                          {own.map((e, n) =>
                            cell(e.id, { text: emojiToken(e), name: e.name }, <img src={e.url} alt="" draggable={false} className="size-7 object-contain" />, n),
                          )}
                        </div>
                      </section>
                    )}
                    {plain.length > 0 && (
                      <section>
                        <p className="sticky top-0 z-10 bg-popover/95 px-1 pb-1 text-[0.65rem] font-extrabold tracking-wide text-muted-foreground uppercase backdrop-blur">
                          Everyday
                        </p>
                        <div className="grid grid-cols-7 gap-0.5">{plain.map((e, n) => cell(e.char, { text: e.char, name: e.names[0]! }, e.char, own.length + n))}</div>
                      </section>
                    )}
                    {own.length === 0 && plain.length === 0 && <p className="py-10 text-center text-sm text-muted-foreground">No emoji called “{q}”.</p>}
                  </div>
                  <div className="flex h-10 items-center gap-2 border-t px-3 text-sm">
                    <AnimatePresence mode="popLayout" initial={false}>
                      <motion.span
                        key={hovered?.text ?? "none"}
                        initial={{ opacity: 0, y: 6 }}
                        animate={{ opacity: 1, y: 0 }}
                        exit={{ opacity: 0, y: -6 }}
                        transition={SPRING}
                        className={cn("truncate", hovered ? "font-bold" : "text-muted-foreground")}
                      >
                        {hovered ? `:${hovered.name}:` : "Pick one, or type : in a message"}
                      </motion.span>
                    </AnimatePresence>
                  </div>
                </motion.div>
              </div>
            </FloatingFocusManager>
          )}
        </AnimatePresence>
      </FloatingPortal>
    </>
  );
}
