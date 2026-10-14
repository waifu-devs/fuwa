import { CalendarIcon, MessageSquareIcon, PaperclipIcon } from "lucide-react";
import { m as motion } from "motion/react";
import { memo, type KeyboardEvent } from "react";
import type { BoardColumn, BoardLabel, Card, Message, User } from "@/gen/fuwa/v1/types_pb";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { useI18n } from "@/i18n/react";
import { colorCss, dueState, ms } from "@/lib/board";
import { displayName } from "@/lib/format";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";

/** A label as a small chip in its color. */
export function LabelChip({ label, className }: { label: BoardLabel; className?: string }) {
  const color = colorCss(label.color);
  return (
    <span
      className={cn("inline-flex max-w-full items-center truncate rounded-full px-2 py-px text-[0.68rem] font-bold", className)}
      style={color ? { background: `color-mix(in srgb, ${color} 22%, transparent)`, color: `color-mix(in oklch, ${color} 75%, var(--foreground))` } : undefined}
    >
      {label.name}
    </span>
  );
}

/** People on a card, overlapping. */
export function Assignees({ users, max = 3, className }: { users: (User | undefined)[]; max?: number; className?: string }) {
  const shown = users.slice(0, max);
  return (
    <span className={cn("flex -space-x-1.5", className)}>
      {shown.map((u, i) => (
        <span key={u?.id ?? i} title={displayName(u)} className="rounded-full ring-2 ring-card">
          <UserAvatar user={u} className="size-5 text-[0.55rem]" />
        </span>
      ))}
      {users.length > max && <span className="grid size-5 place-items-center rounded-full bg-muted text-[0.6rem] font-bold ring-2 ring-card">+{users.length - max}</span>}
    </span>
  );
}

/** A due date, red once it's past (unless the card is done). */
export function DueDate({ card, column, now }: { card: Card; column: BoardColumn | undefined; now: number }) {
  const { date } = useI18n();
  const state = dueState(card, column, now);
  if (!state) return null;
  return (
    <span
      className={cn(
        "flex items-center gap-1 rounded-full px-1.5 py-px",
        state === "late" && "bg-destructive/15 font-bold text-destructive",
        state === "soon" && "bg-primary/15 font-bold text-primary",
      )}
    >
      <CalendarIcon className="size-3" />
      {date(new Date(ms(card.dueAt)), { month: "short", day: "numeric" })}
    </span>
  );
}

/**
 * A card on the board: its labels, title, due date, comments, files and
 * people. It glides to its new place when moved (its `layoutId`), and
 * takes Alt+arrows to move with the keyboard.
 */
export const BoardCard = memo(function BoardCard({
  card,
  body,
  column,
  labels,
  people,
  now,
  selected,
  onOpen,
  onStep,
}: {
  card: Card;
  body: Message | undefined;
  column: BoardColumn | undefined;
  labels: BoardLabel[];
  people: (User | undefined)[];
  now: number;
  selected: boolean;
  onOpen: (id: string) => void;
  onStep?: (id: string, way: "up" | "down" | "left" | "right") => void;
}) {
  const { t } = useI18n();
  const comments = body?.thread?.replyCount ?? 0;
  const files = body?.attachments.length ?? 0;
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onOpen(card.id);
      return;
    }
    if (!e.altKey || !onStep) return;
    const way = ({ ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" } as const)[e.key as "ArrowUp"];
    if (!way) return;
    e.preventDefault();
    onStep(card.id, way);
  };
  return (
    <motion.div
      layout="position"
      layoutId={`card-${card.id}`}
      initial={{ opacity: 0, y: 8, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.94 }}
      transition={SPRING}
      data-card
      data-id={card.id}
      role="button"
      tabIndex={0}
      aria-label={card.title}
      aria-keyshortcuts={onStep ? "Alt+ArrowUp Alt+ArrowDown Alt+ArrowLeft Alt+ArrowRight" : undefined}
      onClick={() => onOpen(card.id)}
      onKeyDown={onKeyDown}
      whileHover={{ y: -2 }}
      className={cn(
        "group cursor-grab touch-pan-y rounded-xl border bg-card p-2.5 text-left shadow-sm transition-[border-color,box-shadow] outline-none select-none hover:border-primary/50 hover:shadow-md focus-visible:ring-2 focus-visible:ring-primary/60 active:cursor-grabbing",
        selected && "border-primary/70 ring-2 ring-primary/30",
      )}
    >
      {labels.length > 0 && (
        <div className="mb-1.5 flex flex-wrap gap-1">
          {labels.map((l) => (
            <LabelChip key={l.id} label={l} />
          ))}
        </div>
      )}
      <p className="text-sm leading-snug font-bold break-words">{card.title}</p>
      {(card.dueAt || comments > 0 || files > 0 || people.length > 0) && (
        <div className="mt-2 flex items-center gap-2 text-[0.7rem] text-muted-foreground">
          <DueDate card={card} column={column} now={now} />
          {comments > 0 && (
            <span className="flex items-center gap-1" title={t("chattools.board.comments", { count: comments })}>
              <MessageSquareIcon className="size-3" /> <Count value={comments} />
            </span>
          )}
          {files > 0 && (
            <span className="flex items-center gap-1" title={t("chattools.board.files", { count: files })}>
              <PaperclipIcon className="size-3" /> {files}
            </span>
          )}
          <span className="flex-1" />
          <Assignees users={people} />
        </div>
      )}
    </motion.div>
  );
});
