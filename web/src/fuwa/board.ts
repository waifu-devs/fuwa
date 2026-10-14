import { useEffect, useSyncExternalStore } from "react";
import { create } from "@bufbuild/protobuf";
import { timestampFromMs } from "@bufbuild/protobuf/wkt";
import { CardChangeSchema, type CardChange } from "@/gen/fuwa/v1/board_pb";
import type { Attachment, BoardColumn, BoardLabel, Card, Emoji, Event, Message, User } from "@/gen/fuwa/v1/types_pb";
import { instanceHas } from "@/lib/compat";
import { applyBoard, BOARD_EVENTS, boardChannelOf, boardOf, EMPTY_BOARD, moved, withCard, type BoardState } from "@/lib/board";
import { reportUsage } from "@/lib/reports";
import { toFuwaError } from "./errors";
import { store, updateInstance, useFuwa, withUsers } from "./store";
import { engine } from "./sync";

/*
 * Boards (docs/boards.md): each open board's columns, labels, cards and
 * card bodies, kept apart from the main store like the economy. A board is
 * read whenever it's shown (its events may have been missed while it was
 * away) and kept in step by its events after that; the calls below show
 * their change at once, and put it back if the server says no.
 */

const call = async <A>(work: () => Promise<A>): Promise<A> => {
  try {
    return await work();
  } catch (err) {
    throw toFuwaError(err);
  }
};

const api = (instanceKey: string) => engine(instanceKey).api.boards;

/** Whether an instance has boards, so boards may be made there. */
export const boardsHere = (instanceKey: string) => instanceHas(store.get().instances[instanceKey]?.node?.versions, "boards");
export const useBoardsHere = (instanceKey: string) => useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "boards"));

// ── The state, per board ─────────────────────────────────────────────────

export type BoardView = BoardState & { loaded: boolean; error: string | null };
const NOT_LOADED: BoardView = { ...EMPTY_BOARD, loaded: false, error: null };

const states = new Map<string, BoardView>();
const listeners = new Set<() => void>();
const subscribe = (l: () => void) => (listeners.add(l), () => listeners.delete(l));
const keyOf = (instanceKey: string, channelId: string) => `${instanceKey}|${channelId}`;

export const boardNow = (instanceKey: string, channelId: string): BoardView => states.get(keyOf(instanceKey, channelId)) ?? NOT_LOADED;

function update(instanceKey: string, channelId: string, fn: (s: BoardView) => BoardView) {
  const key = keyOf(instanceKey, channelId);
  const had = states.get(key) ?? NOT_LOADED;
  const next = fn(had);
  if (next === had) return;
  states.set(key, next);
  for (const l of listeners) l();
}

/** People a board names, into the instance's users so names and avatars show. */
const learn = (instanceKey: string, users: User[]) => {
  if (users.length) updateInstance(instanceKey, (i) => ({ ...i, users: withUsers(i.users, users) }));
};

/** Reads a board. */
export async function loadBoard(instanceKey: string, serverId: string, channelId: string) {
  try {
    const res = await call(() => api(instanceKey).getBoard({ serverId, channelId }));
    learn(instanceKey, res.users);
    update(instanceKey, channelId, () => ({ ...boardOf(res.columns, res.labels, res.cards, res.bodies), loaded: true, error: null }));
  } catch (err) {
    update(instanceKey, channelId, (s) => ({ ...s, error: (err as Error).message }));
  }
}

/** A board, read each time it comes on screen. */
export function useBoard(instanceKey: string, serverId: string, channelId: string): BoardView {
  const state = useSyncExternalStore(subscribe, () => boardNow(instanceKey, channelId));
  useEffect(() => {
    void loadBoard(instanceKey, serverId, channelId);
  }, [instanceKey, serverId, channelId]);
  return state;
}

/** A board's events (and its messages'), for the boards kept here. */
export function onBoardEvent(instanceKey: string, event: Event) {
  const p = event.payload;
  if (!p.case || !BOARD_EVENTS.has(p.case)) return;
  const channelId = boardChannelOf(p);
  if (!channelId || !states.has(keyOf(instanceKey, channelId))) return;
  update(instanceKey, channelId, (s) => {
    const next = applyBoard(s, p);
    return next === s ? s : { ...s, ...next };
  });
}

/** Forgets an instance's boards, when it's removed or signed out of. */
export function forgetBoards(instanceKey: string) {
  for (const key of [...states.keys()]) if (key.startsWith(`${instanceKey}|`)) states.delete(key);
  for (const l of listeners) l();
}

/** Runs a change shown at once, putting the board back if the server refuses it. */
async function optimistic<A>(instanceKey: string, channelId: string, show: (s: BoardState) => BoardState, work: () => Promise<A>): Promise<A> {
  const before = boardNow(instanceKey, channelId);
  update(instanceKey, channelId, (s) => ({ ...s, ...show(s) }));
  try {
    return await call(work);
  } catch (err) {
    update(instanceKey, channelId, (s) => (s === before ? s : { ...s, cards: before.cards, bodies: before.bodies }));
    throw err;
  }
}

// ── Calls ────────────────────────────────────────────────────────────────

export type NewCard = {
  columnId: string;
  title: string;
  content: string;
  attachments: Attachment[];
  emojis: Emoji[];
  labelIds?: string[];
  assigneeIds?: string[];
  dueAt?: number;
  beforeId?: string;
};

export async function createCard(instanceKey: string, serverId: string, channelId: string, card: NewCard): Promise<Card> {
  reportUsage("board.card_create");
  const res = await call(() =>
    api(instanceKey).createCard({
      serverId,
      channelId,
      columnId: card.columnId,
      title: card.title,
      content: card.content,
      attachments: card.attachments,
      emojis: card.emojis,
      labelIds: card.labelIds ?? [],
      assigneeIds: card.assigneeIds ?? [],
      dueAt: card.dueAt ? timestampFromMs(card.dueAt) : undefined,
      beforeId: card.beforeId ?? "",
    }),
  );
  if (res.card) update(instanceKey, channelId, (s) => ({ ...s, ...withCard(s, res.card!, res.body) }));
  return res.card!;
}

export function moveCard(instanceKey: string, serverId: string, channelId: string, cardId: string, columnId: string, beforeId: string) {
  reportUsage("board.card_move");
  return optimistic(
    instanceKey,
    channelId,
    (s) => moved(s, cardId, columnId, beforeId),
    () => api(instanceKey).moveCard({ serverId, channelId, cardId, columnId, beforeId }),
  ).then((res) => {
    if (res.card) update(instanceKey, channelId, (s) => ({ ...s, ...withCard(s, res.card!) }));
  });
}

/** What a card's editor changes; anything left out stays. */
export type CardEdit = {
  title?: string;
  /** null takes the due date off. */
  dueAt?: number | null;
  labelIds?: string[];
  assigneeIds?: string[];
  archived?: boolean;
  body?: { content: string; emojis: Emoji[]; attachments: Attachment[] };
};

export const cardChange = (edit: CardEdit): CardChange =>
  create(CardChangeSchema, {
    title: edit.title,
    setDue: edit.dueAt !== undefined,
    dueAt: edit.dueAt ? timestampFromMs(edit.dueAt) : undefined,
    setLabels: edit.labelIds !== undefined,
    labelIds: edit.labelIds ?? [],
    setAssignees: edit.assigneeIds !== undefined,
    assigneeIds: edit.assigneeIds ?? [],
    archived: edit.archived,
    setBody: !!edit.body,
    content: edit.body?.content ?? "",
    emojis: edit.body?.emojis ?? [],
    attachments: edit.body?.attachments ?? [],
  });

export function updateCard(instanceKey: string, serverId: string, channelId: string, card: Card, edit: CardEdit) {
  if (edit.archived !== undefined) reportUsage(edit.archived ? "board.card_archive" : "board.card_restore");
  const shown: Card = {
    ...card,
    ...(edit.title !== undefined && { title: edit.title }),
    ...(edit.labelIds && { labelIds: edit.labelIds }),
    ...(edit.assigneeIds && { assigneeIds: edit.assigneeIds }),
    ...(edit.dueAt !== undefined && { dueAt: edit.dueAt ? timestampFromMs(edit.dueAt) : undefined }),
    ...(edit.archived !== undefined && { archivedAt: edit.archived ? timestampFromMs(Date.now()) : undefined }),
  };
  return optimistic(
    instanceKey,
    channelId,
    (s) => withCard(s, shown),
    () => api(instanceKey).updateCard({ serverId, channelId, cardId: card.id, change: cardChange(edit) }),
  ).then((res) => {
    if (res.card) update(instanceKey, channelId, (s) => ({ ...s, ...withCard(s, res.card!, res.body) }));
    return res.card;
  });
}

export async function setBoard(instanceKey: string, serverId: string, channelId: string, columns: BoardColumn[], labels: BoardLabel[]) {
  const res = await call(() => api(instanceKey).setBoard({ serverId, channelId, columns, labels }));
  update(instanceKey, channelId, (s) => ({ ...s, columns: res.columns, labels: res.labels }));
}

/** A page of a board's archived cards, the latest archived first. */
export async function listArchived(instanceKey: string, serverId: string, channelId: string, afterId = "") {
  const res = await call(() => api(instanceKey).listArchivedCards({ serverId, channelId, afterId, limit: 50 }));
  learn(instanceKey, res.users);
  const bodies: Record<string, Message> = Object.fromEntries(res.bodies.map((m) => [m.id, m]));
  return { cards: res.cards, bodies, hasMore: res.hasMore };
}
