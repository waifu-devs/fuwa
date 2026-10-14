import type { BoardColumn, BoardLabel, Card, Event, Message, ThreadSummary } from "@/gen/fuwa/v1/types_pb";
import type { Timestamp } from "@bufbuild/protobuf/wkt";

/*
 * Boards (docs/boards.md): the decisions a board's screen makes, kept pure
 * so node's test runner reads this file as is (type imports only). A board
 * is its columns, labels and cards; each card's body is a message, kept
 * beside the card. Events keep it in step (`applyBoard`), and a move shows
 * at once with a rank worked out here the way the server works it out
 * (`between`, server/src/api/boards.rs), so the card stays where it was
 * dropped until the server's answer comes back.
 */

/** As the server takes them. */
export const TITLE_MAX = 200;
export const COLUMN_NAME_MAX = 50;
export const LABEL_NAME_MAX = 30;
export const COLUMNS_MAX = 20;
export const LABELS_MAX = 50;
export const ASSIGNEES_MAX = 10;

/** A board as one app holds it. */
export type BoardState = {
  columns: BoardColumn[];
  labels: BoardLabel[];
  /** Not archived, sorted by rank. */
  cards: Card[];
  /** Each card's body, by the card's id. */
  bodies: Record<string, Message>;
};

export const EMPTY_BOARD: BoardState = { columns: [], labels: [], cards: [], bodies: {} };

const DIGITS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE = DIGITS.length;

function midpoint(lo: number[], hi: number[] | null): number[] {
  if (hi) {
    let shared = 0;
    while (shared < hi.length && (lo[shared] ?? 0) === hi[shared]) shared++;
    if (shared > 0) return [...hi.slice(0, shared), ...midpoint(lo.slice(shared), hi.slice(shared))];
  }
  const a = lo[0] ?? 0;
  const b = hi ? hi[0]! : BASE;
  if (b - a > 1) return [Math.floor((a + b) / 2)];
  if (hi && hi.length > 1) return [hi[0]!];
  return [a, ...midpoint(lo.slice(1), null)];
}

/** A rank between `lo` and `hi`; "" is before everything (lo) or after everything (hi). */
export function between(lo: string, hi: string): string {
  const digits = (s: string) => [...s].map((c) => Math.max(0, DIGITS.indexOf(c)));
  return midpoint(digits(lo), hi ? digits(hi) : null)
    .map((d) => DIGITS[d])
    .join("");
}

/** How cards sort in a column: by rank, then id, as the server sorts them. */
export const byRank = (a: Card, b: Card) => (a.rank < b.rank ? -1 : a.rank > b.rank ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0);

/** A column's cards, in order. */
export const cardsIn = (board: BoardState, columnId: string) => board.cards.filter((c) => c.columnId === columnId);

/** Unix milliseconds of a timestamp, 0 for none. */
export const ms = (t: Timestamp | undefined) => (t ? Number(t.seconds) * 1000 + Math.floor(t.nanos / 1e6) : 0);

const put = (cards: Card[], card: Card) => [...cards.filter((c) => c.id !== card.id), ...(ms(card.archivedAt) ? [] : [card])].sort(byRank);

const without = (bodies: Record<string, Message>, id: string) => {
  if (!(id in bodies)) return bodies;
  const { [id]: _, ...rest } = bodies;
  return rest;
};

/** A board as it was read (GetBoard). */
export const boardOf = (columns: BoardColumn[], labels: BoardLabel[], cards: Card[], bodies: Message[]): BoardState => ({
  columns,
  labels,
  cards: [...cards].sort(byRank),
  bodies: Object.fromEntries(bodies.map((m) => [m.id, m])),
});

/** Puts a card in (or takes it off, archived), with its body when there is one. */
export function withCard(board: BoardState, card: Card, body?: Message): BoardState {
  return { ...board, cards: put(board.cards, card), bodies: body ? { ...board.bodies, [card.id]: body } : board.bodies };
}

/** The event kinds a board follows. */
export const BOARD_EVENTS = new Set<string>([
  "boardUpdated",
  "cardUpdated",
  "cardDeleted",
  "messageCreated",
  "messageUpdated",
  "messageDeleted",
  "threadUpdated",
]);

/** The channel a board event is about, if it's one a board follows. */
export function boardChannelOf(p: Event["payload"]): string | null {
  switch (p.case) {
    case "boardUpdated":
    case "cardDeleted":
    case "threadUpdated":
    case "messageDeleted":
      return p.value.channelId;
    case "cardUpdated":
      return p.value.card?.channelId ?? null;
    case "messageCreated":
    case "messageUpdated":
      return p.value.message?.channelId ?? null;
    default:
      return null;
  }
}

/** A board after one of its channel's events. Events can come twice, so each one only sets what it says. */
export function applyBoard(board: BoardState, p: Event["payload"]): BoardState {
  switch (p.case) {
    case "boardUpdated": {
      const labels = new Set(p.value.labels.map((l) => l.id));
      const cards = board.cards.map((c) => (c.labelIds.every((id) => labels.has(id)) ? c : { ...c, labelIds: c.labelIds.filter((id) => labels.has(id)) }));
      return { ...board, columns: p.value.columns, labels: p.value.labels, cards };
    }
    case "cardUpdated":
      return p.value.card ? withCard(board, p.value.card) : board;
    case "cardDeleted":
      return { ...board, cards: board.cards.filter((c) => c.id !== p.value.cardId), bodies: without(board.bodies, p.value.cardId) };
    case "messageCreated":
    case "messageUpdated": {
      const m = p.value.message;
      // A body: a message on the board that isn't a comment. Comments only change their card's thread.
      if (!m || m.threadId) return board;
      const had = board.bodies[m.id];
      return { ...board, bodies: { ...board.bodies, [m.id]: { ...m, thread: m.thread ?? had?.thread, reactions: had?.reactions ?? m.reactions } } };
    }
    case "messageDeleted":
      if (!(p.value.messageId in board.bodies) && !board.cards.some((c) => c.id === p.value.messageId)) return board;
      return { ...board, cards: board.cards.filter((c) => c.id !== p.value.messageId), bodies: without(board.bodies, p.value.messageId) };
    case "threadUpdated": {
      const body = board.bodies[p.value.threadId];
      if (!body) return board;
      return { ...board, bodies: { ...board.bodies, [body.id]: { ...body, thread: p.value.thread as ThreadSummary | undefined } } };
    }
    default:
      return board;
  }
}

/**
 * Where a card lands when dropped above `beforeId` in a column (at the
 * bottom when it's empty): the rank the server will give it, worked out
 * from the cards there now.
 */
export function rankFor(board: BoardState, columnId: string, beforeId: string, moving: string): string {
  const cards = cardsIn(board, columnId).filter((c) => c.id !== moving);
  const at = beforeId ? cards.findIndex((c) => c.id === beforeId) : cards.length;
  const index = at < 0 ? cards.length : at;
  const lo = index > 0 ? cards[index - 1]!.rank : "";
  const hi = cards[index]?.rank ?? "";
  // Two cards sharing a rank (moved there at once): the server spreads them out.
  return hi && lo >= hi ? lo : between(lo, hi);
}

/** A board with a card moved, as it shows until the server answers. */
export function moved(board: BoardState, cardId: string, columnId: string, beforeId: string): BoardState {
  const card = board.cards.find((c) => c.id === cardId);
  if (!card) return board;
  return withCard(board, { ...card, columnId, rank: rankFor(board, columnId, beforeId, cardId) });
}

/**
 * Where a card goes with the keyboard: up or down a place in its column,
 * or to the top of the column beside it. `null` when it can't go that way.
 */
export function step(board: BoardState, cardId: string, way: "up" | "down" | "left" | "right"): { columnId: string; beforeId: string } | null {
  const card = board.cards.find((c) => c.id === cardId);
  if (!card) return null;
  const here = cardsIn(board, card.columnId);
  const i = here.findIndex((c) => c.id === cardId);
  if (way === "up") return i > 0 ? { columnId: card.columnId, beforeId: here[i - 1]!.id } : null;
  if (way === "down") {
    if (i >= here.length - 1) return null;
    return { columnId: card.columnId, beforeId: here[i + 2]?.id ?? "" };
  }
  const c = board.columns.findIndex((col) => col.id === card.columnId);
  const next = board.columns[way === "left" ? c - 1 : c + 1];
  if (!next) return null;
  return { columnId: next.id, beforeId: cardsIn(board, next.id)[0]?.id ?? "" };
}

/** What narrows the board: cards on someone, with a label, or saying something. */
export type BoardFilter = { assignee?: string; labelId?: string; query?: string };

/** Whether a card shows under a filter: its title or body says every word. */
export function shows(card: Card, body: Message | undefined, filter: BoardFilter): boolean {
  if (filter.assignee && !card.assigneeIds.includes(filter.assignee)) return false;
  if (filter.labelId && !card.labelIds.includes(filter.labelId)) return false;
  const words = (filter.query ?? "").toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const text = `${card.title}\n${body?.content ?? ""}`.toLowerCase();
  return words.every((w) => text.includes(w));
}

/** How a card's due date reads: past and not done, within a day, or later. */
export function dueState(card: Card, column: BoardColumn | undefined, now: number): "late" | "soon" | "later" | null {
  const due = ms(card.dueAt);
  if (!due) return null;
  if (column?.done) return "later";
  if (due < now) return "late";
  return due - now < 86_400_000 ? "soon" : "later";
}

/** Whether a column holds more cards than its limit says. */
export const overLimit = (board: BoardState, column: BoardColumn) => column.cardLimit > 0 && cardsIn(board, column.id).length > column.cardLimit;

/** `0xRRGGBB` as CSS, or none for 0. */
export const colorCss = (color: number) => (color ? `#${color.toString(16).padStart(6, "0")}` : undefined);

/** The colors the board's settings offer for columns and labels. */
export const PALETTE = [0xf06292, 0xb388ff, 0x7986cb, 0x4fc3f7, 0x4db6ac, 0x81c784, 0xdce775, 0xffd54f, 0xffb74d, 0xff8a65, 0xa1887f, 0x90a4ae];
