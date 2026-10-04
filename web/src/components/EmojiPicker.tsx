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
import {
  ClockIcon,
  FlagIcon,
  HeartIcon,
  LeafIcon,
  LightbulbIcon,
  PizzaIcon,
  PlaneIcon,
  SearchIcon,
  SmileIcon,
  TrophyIcon,
  type LucideIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import {
  memo,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactElement,
  type RefObject,
  type UIEvent,
} from "react";
import type { Emoji } from "@/gen/fuwa/v1/types_pb";
import { EmojiImage } from "@/components/EmojiImage";
import { ServerIcon } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { emojiToken } from "@/lib/emoji";
import {
  choiceName,
  customChoice,
  ownCatalog,
  rememberEmoji,
  searchCatalog,
  setSkinTone,
  standardChoice,
  toned,
  recentEmoji,
  useSkinTone,
  useStandard,
  type Catalog,
  type Choice,
  type CustomEmoji,
  type ServerRef,
  type StandardEmoji,
} from "@/lib/emoji-catalog";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { reportUsage } from "@/lib/reports";
import { cn } from "@/lib/utils";

/** What a pick gives: the text to put in (a Unicode emoji, or `<:name:id>`), and the name it's written by after a colon. */
export type PickedEmoji = { text: string; name: string; custom?: CustomEmoji };

/** Grid geometry, in pixels: rows are laid out by hand so only the ones on screen are drawn. */
const COLS = 8;
const CELL = 40;
const HEADER = 30;
const PAD = 8;
/** Rows drawn above and below the visible ones, so fast scrolling never shows a gap. */
const OVERSCAN = 4;

const GROUP_ICONS: Record<string, LucideIcon> = {
  people: SmileIcon,
  nature: LeafIcon,
  food: PizzaIcon,
  activities: TrophyIcon,
  travel: PlaneIcon,
  objects: LightbulbIcon,
  symbols: HeartIcon,
  flags: FlagIcon,
};
const TONES = ["✋", "✋🏻", "✋🏼", "✋🏽", "✋🏾", "✋🏿"];
const TONE_NAMES = ["Default", "Light", "Medium-light", "Medium", "Medium-dark", "Dark"];

type Section = { id: string; title: string; server?: ServerRef; icon?: LucideIcon; choices: Choice[] };
type Row =
  | { kind: "header"; key: string; top: number; section: Section }
  | { kind: "cells"; key: string; top: number; start: number; choices: Choice[] };
type Layout = { rows: Row[]; height: number; flat: Choice[]; sectionTops: { id: string; top: number }[]; cellRow: number[] };

function layOut(sections: Section[]): Layout {
  const rows: Row[] = [];
  const flat: Choice[] = [];
  const sectionTops: Layout["sectionTops"] = [];
  const cellRow: number[] = [];
  let top = 0;
  for (const section of sections) {
    if (!section.choices.length) continue;
    sectionTops.push({ id: section.id, top });
    rows.push({ kind: "header", key: `h:${section.id}`, top, section });
    top += HEADER;
    for (let i = 0; i < section.choices.length; i += COLS) {
      const choices = section.choices.slice(i, i + COLS);
      const row = rows.length;
      rows.push({ kind: "cells", key: `r:${section.id}:${i}`, top, start: flat.length, choices });
      for (const choice of choices) {
        flat.push(choice);
        cellRow.push(row);
      }
      top += CELL;
    }
  }
  return { rows, height: top, flat, sectionTops, cellRow };
}

/** The first row whose bottom is below `y`. */
function rowAt(rows: Row[], y: number) {
  let lo = 0;
  let hi = rows.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    const r = rows[mid]!;
    if (r.top + (r.kind === "header" ? HEADER : CELL) <= y) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * A grid of emoji that opens from `trigger`: recently used ones, the
 * server's own, the person's other servers' (each under its icon), then the
 * standard set by category, with skin tones. Search matches names and the
 * words people use for them. Only the rows on screen are drawn, so
 * thousands of emoji scroll smoothly; arrows move through them from the
 * search box and Enter picks.
 */
export function EmojiPicker({
  catalog,
  emojis,
  server,
  onPick,
  children,
  placement = "top-end",
  closeOnPick = true,
}: {
  /** Every server emoji to offer (see `useCatalog`). */
  catalog?: Catalog;
  /** Or just one server's own. */
  emojis?: Emoji[];
  server?: ServerRef;
  onPick: (emoji: PickedEmoji) => void;
  /** The button that opens it. */
  children: (open: boolean) => ReactElement;
  placement?: Placement;
  closeOnPick?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const { refs, floatingStyles, context, elements } = useFloating({
    open,
    onOpenChange: setOpen,
    placement,
    whileElementsMounted: autoUpdate,
    middleware: [offset(8), flip(), shift({ padding: 8 })],
  });
  const { getReferenceProps, getFloatingProps } = useInteractions([useClick(context), useDismiss(context), useRole(context, { role: "dialog" })]);
  const own = useMemo(() => catalog ?? ownCatalog(server, emojis), [catalog, server, emojis]);

  // Inside a dialog it opens in the dialog, which keeps pointer events (and clicks) to itself.
  const dialog = (elements.domReference as Element | null)?.closest<HTMLElement>("[role=dialog]") ?? null;

  const pick = useCallback(
    (picked: PickedEmoji) => {
      onPick(picked);
      if (closeOnPick) setOpen(false);
    },
    [onPick, closeOnPick],
  );

  return (
    <>
      <span ref={refs.setReference} {...getReferenceProps()} className="inline-flex">
        {children(open)}
      </span>
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
                  style={{ transformOrigin: placement.startsWith("bottom") ? "top" : "bottom" }}
                >
                  <PickerPanel catalog={own} onPick={pick} />
                </motion.div>
              </div>
            </FloatingFocusManager>
          )}
        </AnimatePresence>
      </FloatingPortal>
    </>
  );
}

/** What's inside the picker, mounted while it's open. */
function PickerPanel({ catalog, onPick }: { catalog: Catalog; onPick: (picked: PickedEmoji) => void }) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState<number | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [intro, setIntro] = useState(true);
  const groups = useStandard();
  // As they were when it opened, so picking a few in a row doesn't shift the grid under the pointer.
  const [recentKeys] = useState(recentEmoji);
  const tone = useSkinTone();
  const calm = usePrefs(reduceMotion);
  const scroller = useRef<HTMLDivElement>(null);
  const frame = useRef(0);
  const id = useId();
  const searching = query.trim().length > 0;

  useEffect(() => {
    const t = window.setTimeout(() => setIntro(false), 450);
    return () => window.clearTimeout(t);
  }, []);

  const sections = useMemo<Section[]>(() => {
    if (searching) {
      const found = searchCatalog(query, catalog, groups);
      return [{ id: "results", title: found.length ? `${found.length} found` : "Nothing found", icon: SearchIcon, choices: found }];
    }
    const byChar = new Map<string, StandardEmoji>();
    for (const g of groups ?? []) for (const e of g.emojis) byChar.set(e.char, e);
    const recent = recentKeys.flatMap((key): Choice[] => {
      const custom = key.startsWith("c:") ? catalog.byId.get(key.slice(2)) : undefined;
      if (custom) return [customChoice(custom)];
      const standard = key.startsWith("u:") ? byChar.get(key.slice(2)) : undefined;
      return standard ? [standardChoice(standard)] : [];
    });
    return [
      { id: "recent", title: "Recently used", icon: ClockIcon, choices: recent.slice(0, COLS * 2) },
      ...catalog.sections.map((s) => ({ id: `s:${s.server.id}`, title: s.server.name, server: s.server, choices: s.emojis.map(customChoice) })),
      ...(groups ?? []).map((g) => ({ id: g.id, title: g.name, icon: GROUP_ICONS[g.id], choices: g.emojis.map(standardChoice) })),
    ];
  }, [searching, query, catalog, groups, recentKeys]);

  const layout = useMemo(() => layOut(sections), [sections]);
  const viewport = 18 * 16 - PAD * 2;

  // A new search starts at the top, on its best match.
  useLayoutEffect(() => {
    scroller.current?.scrollTo({ top: 0 });
    setScrollTop(0);
    setActive(searching ? 0 : null);
  }, [query, searching]);

  const onScroll = (e: UIEvent<HTMLDivElement>) => {
    const y = e.currentTarget.scrollTop;
    cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => setScrollTop(y));
  };
  useEffect(() => () => cancelAnimationFrame(frame.current), []);

  const first = Math.max(0, rowAt(layout.rows, scrollTop) - OVERSCAN);
  const last = Math.min(layout.rows.length - 1, rowAt(layout.rows, scrollTop + viewport) + OVERSCAN);
  const visible = layout.rows.slice(first, last + 1);
  const current = [...layout.sectionTops].reverse().find((s) => s.top <= scrollTop + 1) ?? layout.sectionTops[0];
  const currentSection = sections.find((s) => s.id === current?.id);

  const choose = useCallback(
    (choice: Choice) => {
      rememberEmoji(choice.key);
      if (choice.kind === "custom") {
        reportUsage(choice.custom.here ? "emoji.pick.server" : "emoji.pick.other_server");
        onPick({ text: emojiToken(choice.custom.emoji), name: choice.custom.alias, custom: choice.custom });
      } else {
        reportUsage("emoji.pick.standard");
        onPick({ text: toned(choice.emoji, tone), name: choice.emoji.names[0]! });
      }
    },
    [onPick, tone],
  );

  const onKeyDown = useGridKeys(layout, scroller, viewport, active, setActive, query.length, choose);

  // A far section is reached by skipping to a screen away and gliding the
  // rest, so the rows in between never draw (or fetch their pictures).
  function jump(sectionId: string) {
    const el = scroller.current;
    if (!el) return;
    const top = layout.sectionTops.find((s) => s.id === sectionId)?.top ?? 0;
    if (calm) return el.scrollTo({ top });
    const from = el.scrollTop;
    if (Math.abs(top - from) > viewport * 2) el.scrollTop = top + (top > from ? -viewport : viewport);
    el.scrollTo({ top, behavior: "smooth" });
  }

  const shown = active !== null ? layout.flat[active] : undefined;
  const activeRow = active !== null ? layout.rows[layout.cellRow[active] ?? -1] : undefined;
  const activeCol = active !== null && activeRow?.kind === "cells" ? active - activeRow.start : 0;
  const listId = `${id}-emoji`;

  return (
    <div className="flex w-[22rem] max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-2xl border bg-popover shadow-2xl">
      <div className="flex items-center gap-1.5 border-b p-2">
        <div className="relative flex-1">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Find an emoji"
            aria-label="Find an emoji"
            role="combobox"
            aria-expanded
            aria-controls={listId}
            aria-activedescendant={active !== null ? `${listId}-${active}` : undefined}
            className="h-9 w-full rounded-xl bg-muted/60 pr-3 pl-8 text-sm outline-none focus:ring-2 focus:ring-primary/40"
          />
        </div>
        <ToneButton tone={tone} />
      </div>

      {!searching && <CategoryRail sections={sections} tops={layout.sectionTops} current={current?.id} layoutId={`${id}-rail`} onJump={jump} />}

      <div className="relative">
        <AnimatePresence initial={false}>
          {currentSection && layout.rows.length > 0 && (
            <motion.p
              key={currentSection.id}
              initial={{ opacity: 0, y: -6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: 6 }}
              transition={{ duration: 0.16 }}
              aria-hidden
              className="pointer-events-none absolute inset-x-0 top-0 z-10 flex h-[30px] items-center gap-1.5 bg-popover px-3 text-[0.65rem] font-extrabold tracking-wide text-muted-foreground uppercase"
            >
              <SectionLabel section={currentSection} />
            </motion.p>
          )}
        </AnimatePresence>
        <div ref={scroller} onScroll={onScroll} className="scroll-thin h-72 overflow-y-auto overscroll-contain" style={{ padding: `0 ${PAD}px ${PAD}px` }}>
          <motion.div
            key={searching ? "results" : "all"}
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.18 }}
            id={listId}
            role="listbox"
            aria-label="Emoji"
            className="relative"
            style={{ height: Math.max(layout.height, 40) }}
          >
            {shown && activeRow && (
              <motion.span
                aria-hidden
                initial={false}
                animate={{ x: activeCol * CELL, y: activeRow.top }}
                transition={calm ? { duration: 0 } : { type: "spring", stiffness: 700, damping: 40 }}
                className="absolute top-0 left-0 size-10 rounded-lg bg-primary/12"
              />
            )}
            {visible.map((row) =>
              row.kind === "header" ? (
                <p
                  key={row.key}
                  className="absolute inset-x-0 flex h-[30px] items-center gap-1.5 px-1 text-[0.65rem] font-extrabold tracking-wide text-muted-foreground uppercase"
                  style={{ transform: `translateY(${row.top}px)` }}
                >
                  <SectionLabel section={row.section} />
                </p>
              ) : (
                <CellRow
                  key={row.key}
                  row={row}
                  tone={tone}
                  active={active !== null && active >= row.start && active < row.start + row.choices.length ? active - row.start : -1}
                  intro={intro && !calm}
                  listId={listId}
                  onHover={setActive}
                  onChoose={choose}
                />
              ),
            )}
          </motion.div>
          {searching && !layout.flat.length && (
            <p className="py-10 text-center text-sm text-muted-foreground">No emoji called “{query.trim().replace(/^:|:$/g, "")}”.</p>
          )}
          {!groups && !searching && <p className="py-3 text-center text-xs text-muted-foreground">Loading the rest…</p>}
        </div>
      </div>

      <Preview shown={shown} tone={tone} />
    </div>
  );
}

function SectionLabel({ section }: { section: Section }) {
  const Icon = section.icon;
  return (
    <>
      {section.server ? <ServerIcon server={section.server} className="size-4 rounded-md text-[0.4rem]" /> : Icon && <Icon className="size-3" />}
      <span className="truncate">{section.title}</span>
    </>
  );
}

/** One row of emoji; only redrawn when what's in it, the tone, or the active one in it changes. */
const CellRow = memo(function CellRow({
  row,
  tone,
  active,
  intro,
  listId,
  onHover,
  onChoose,
}: {
  row: Extract<Row, { kind: "cells" }>;
  tone: number;
  active: number;
  intro: boolean;
  listId: string;
  onHover: (index: number) => void;
  onChoose: (choice: Choice) => void;
}) {
  return (
    <div className="absolute inset-x-0 flex" style={{ transform: `translateY(${row.top}px)` }}>
      {row.choices.map((choice, col) => {
        const index = row.start + col;
        return (
          <button
            key={choice.key}
            id={`${listId}-${index}`}
            type="button"
            role="option"
            aria-selected={col === active}
            aria-label={`:${choiceName(choice)}:`}
            tabIndex={-1}
            onPointerEnter={() => onHover(index)}
            onClick={() => onChoose(choice)}
            className={cn("emoji-cell grid size-10 place-items-center rounded-lg text-[1.6rem] leading-none", intro && "emoji-cell-in")}
            style={intro ? { animationDelay: `${Math.min(index, 40) * 8}ms` } : undefined}
          >
            <Glyph choice={choice} tone={tone} playing={col === active} className="size-7" />
          </button>
        );
      })}
    </div>
  );
});

function Glyph({ choice, tone, playing, className }: { choice: Choice; tone: number; playing?: boolean; className?: string }) {
  if (choice.kind === "custom") return <EmojiImage emoji={choice.custom.emoji} playing={playing} title={false} className={className} />;
  return <span aria-hidden>{toned(choice.emoji, tone)}</span>;
}

/**
 * Arrow keys from the search box: left and right step through the emoji
 * (once the caret is at the end of the search), up and down move a row in
 * the same column, skipping headers; Enter picks the one that's lit, or the
 * first.
 */
function useGridKeys(
  layout: Layout,
  scroller: RefObject<HTMLDivElement | null>,
  viewport: number,
  active: number | null,
  setActive: (index: number) => void,
  queryLength: number,
  choose: (choice: Choice) => void,
) {
  /** Moves to an emoji and scrolls it into view. */
  const move = (to: number) => {
    const next = Math.max(0, Math.min(layout.flat.length - 1, to));
    setActive(next);
    const el = scroller.current;
    const row = layout.rows[layout.cellRow[next] ?? -1];
    if (!el || !row) return;
    if (row.top < el.scrollTop + HEADER) el.scrollTop = Math.max(0, row.top - HEADER);
    else if (row.top + CELL > el.scrollTop + viewport) el.scrollTop = row.top + CELL - viewport;
  };

  /** The same column in the next (or previous) row of emoji. */
  const vertical = (from: number, by: 1 | -1) => {
    const rowIndex = layout.cellRow[from]!;
    const row = layout.rows[rowIndex] as Extract<Row, { kind: "cells" }>;
    for (let r = rowIndex + by; r >= 0 && r < layout.rows.length; r += by) {
      const next = layout.rows[r]!;
      if (next.kind === "cells") return next.start + Math.min(from - row.start, next.choices.length - 1);
    }
    return from;
  };

  return (e: KeyboardEvent<HTMLInputElement>) => {
    if (!layout.flat.length) return;
    const atEnd = e.currentTarget.selectionStart === queryLength;
    const step: Record<string, (() => number | null) | undefined> = {
      ArrowRight: () => (active === null ? 0 : atEnd ? active + 1 : null),
      ArrowLeft: () => (active !== null && atEnd ? active - 1 : null),
      ArrowDown: () => (active === null ? 0 : vertical(active, 1)),
      ArrowUp: () => (active === null ? null : vertical(active, -1)),
    };
    if (e.key === "Enter") {
      e.preventDefault();
      const choice = layout.flat[active ?? 0];
      if (choice) choose(choice);
      return;
    }
    const to = step[e.key]?.();
    if (to === undefined || to === null) return;
    e.preventDefault();
    move(to);
  };
}

/** The skin tone for standard emoji, picked from a row of hands. */
function ToneButton({ tone }: { tone: number }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="relative">
      <motion.button
        type="button"
        aria-label={`Skin tone: ${TONE_NAMES[tone]}`}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        whileHover={{ scale: 1.12, rotate: -8 }}
        whileTap={{ scale: 0.88 }}
        className="grid size-9 place-items-center rounded-xl text-xl leading-none hover:bg-primary/10"
      >
        {TONES[tone]}
      </motion.button>
      <AnimatePresence>
        {open && (
          <motion.div
            role="radiogroup"
            aria-label="Skin tone"
            initial={{ opacity: 0, scale: 0.85, y: -4 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.9, y: -4 }}
            transition={SPRING}
            style={{ transformOrigin: "top right" }}
            className="absolute top-full right-0 z-20 mt-1 flex gap-0.5 rounded-xl border bg-popover p-1 shadow-xl"
          >
            {TONES.map((hand, n) => (
              <motion.button
                key={hand}
                type="button"
                role="radio"
                aria-checked={n === tone}
                aria-label={TONE_NAMES[n]}
                initial={{ opacity: 0, scale: 0.5 }}
                animate={{ opacity: 1, scale: 1 }}
                transition={{ ...SPRING, delay: n * 0.025 }}
                whileHover={{ scale: 1.2 }}
                onClick={() => {
                  setSkinTone(n);
                  setOpen(false);
                }}
                className={cn("grid size-8 place-items-center rounded-lg text-lg leading-none", n === tone && "bg-primary/15")}
              >
                {hand}
              </motion.button>
            ))}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** A button per section; the one in view is lit, and the light glides along as you scroll. */
function CategoryRail({
  sections,
  tops,
  current,
  layoutId,
  onJump,
}: {
  sections: Section[];
  tops: Layout["sectionTops"];
  current: string | undefined;
  layoutId: string;
  onJump: (sectionId: string) => void;
}) {
  return (
    <nav aria-label="Emoji categories" className="scroll-thin flex gap-0.5 overflow-x-auto border-b px-2 py-1.5">
      {tops.map(({ id }) => {
        const section = sections.find((s) => s.id === id)!;
        const on = current === id;
        const Icon = section.icon;
        return (
          <button
            key={id}
            type="button"
            title={section.title}
            aria-label={section.title}
            aria-current={on || undefined}
            onClick={() => onJump(id)}
            className={cn("relative grid size-8 shrink-0 place-items-center rounded-lg text-muted-foreground transition-colors hover:text-foreground", on && "text-primary")}
          >
            {on && <motion.span layoutId={layoutId} transition={SPRING} className="absolute inset-0 rounded-lg bg-primary/12" />}
            {section.server ? (
              <ServerIcon server={section.server} className="relative size-5 rounded-md text-[0.45rem]" />
            ) : (
              Icon && <Icon className="relative size-4" />
            )}
          </button>
        );
      })}
    </nav>
  );
}

/** The emoji you're on, big, with the name to type and where it's from. */
function Preview({ shown, tone }: { shown: Choice | undefined; tone: number }) {
  return (
    <div className="flex h-12 items-center gap-2.5 border-t px-3 text-sm">
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.div
          key={shown?.key ?? "none"}
          initial={{ opacity: 0, y: 6 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -6 }}
          transition={SPRING}
          className="flex min-w-0 items-center gap-2.5"
        >
          {shown ? (
            <>
              <span className="grid size-8 shrink-0 place-items-center text-2xl leading-none">
                <Glyph choice={shown} tone={tone} playing className="size-8" />
              </span>
              <span className="min-w-0">
                <span className="block truncate font-bold">:{choiceName(shown)}:</span>
                {shown.kind === "custom" && (
                  <span className="block truncate text-xs text-muted-foreground">
                    {shown.custom.here ? "From this server" : `From ${shown.custom.server.name}`}
                  </span>
                )}
              </span>
            </>
          ) : (
            <span className="text-muted-foreground">Pick one, or type : in a message</span>
          )}
        </motion.div>
      </AnimatePresence>
    </div>
  );
}
