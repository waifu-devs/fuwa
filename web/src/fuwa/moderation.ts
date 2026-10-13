import { useEffect, useSyncExternalStore } from "react";
import type {
  FindPeopleResponse,
  GetMemberOverviewResponse,
  GetModCaseResponse,
  ListMemberMessagesResponse,
  ListModCasesResponse,
} from "@/gen/fuwa/v1/moderation_pb";
import type { Event, ModCase, ModNote, ModReportReason, ModCaseStatus, Server } from "@/gen/fuwa/v1/types_pb";
import { reportUsage } from "@/lib/reports";
import { OPEN } from "@/lib/mod-view";
import { toFuwaError } from "./errors";
import { engine } from "./sync";

/*
 * Mod View (docs/mod-view.md): reporting, for everyone, and the calls behind
 * the moderators' screen. Kept apart from the main store: the screen loads
 * what it shows when it opens and patches it from the events below, and the
 * only thing kept here is each server's count of open cases, for the badges
 * in the server's menus.
 */

const PAGE = 50;

const call = async <A>(work: () => Promise<A>): Promise<A> => {
  try {
    return await work();
  } catch (err) {
    throw toFuwaError(err);
  }
};

const api = (instanceKey: string) => engine(instanceKey).api.moderation;

// ── Reporting ────────────────────────────────────────────────────────────

export function reportMessage(instanceKey: string, serverId: string, channelId: string, messageId: string, reason: ModReportReason, note: string) {
  reportUsage("mod.report_message");
  return call(() => api(instanceKey).reportMessage({ serverId, channelId, messageId, reason, note }));
}

export function reportMember(instanceKey: string, serverId: string, userId: string, reason: ModReportReason, note: string) {
  reportUsage("mod.report_member");
  return call(() => api(instanceKey).reportMember({ serverId, userId, reason, note }));
}

// ── The moderators' calls ────────────────────────────────────────────────

export function listCases(
  instanceKey: string,
  serverId: string,
  filter: { status: number; subjectId?: string; beforeId?: string; limit?: number },
): Promise<ListModCasesResponse> {
  return call(async () => {
    const res = await api(instanceKey).listModCases({
      serverId,
      status: filter.status as ModCaseStatus,
      subjectId: filter.subjectId ?? "",
      beforeId: filter.beforeId ?? "",
      limit: filter.limit ?? PAGE,
    });
    setCount(instanceKey, serverId, res.openCount);
    return res;
  });
}

export const getCase = (instanceKey: string, serverId: string, caseId: string): Promise<GetModCaseResponse> =>
  call(() => api(instanceKey).getModCase({ serverId, caseId }));

export async function updateCase(instanceKey: string, serverId: string, caseId: string, status: ModCaseStatus, resolution: string): Promise<ModCase | undefined> {
  reportUsage(status === OPEN ? "mod.case_reopen" : "mod.case_close");
  const res = await call(() => api(instanceKey).updateModCase({ serverId, caseId, status, resolution }));
  recountSoon(instanceKey, serverId);
  return res.modCase;
}

export const memberOverview = (instanceKey: string, serverId: string, userId: string): Promise<GetMemberOverviewResponse> =>
  call(() => api(instanceKey).getMemberOverview({ serverId, userId }));

export const memberMessages = (instanceKey: string, serverId: string, userId: string, beforeId = ""): Promise<ListMemberMessagesResponse> =>
  call(() => api(instanceKey).listMemberMessages({ serverId, userId, beforeId, limit: PAGE, channelId: "" }));

export const findPeople = (instanceKey: string, serverId: string, query: string, signal?: AbortSignal): Promise<FindPeopleResponse> =>
  call(() => api(instanceKey).findPeople({ serverId, query, limit: 20 }, { signal }));

export async function addNote(instanceKey: string, serverId: string, userId: string, content: string): Promise<ModNote | undefined> {
  reportUsage("mod.note_add");
  return (await call(() => api(instanceKey).addModNote({ serverId, userId, content }))).note;
}

export async function deleteNote(instanceKey: string, serverId: string, noteId: string) {
  await call(() => api(instanceKey).deleteModNote({ serverId, noteId }));
}

// ── Open cases per server, for the badges ────────────────────────────────

/** Read again when shown after this long, since events only come while connected. */
const STALE_MS = 60_000;

const counts = new Map<string, { count: number; at: number }>();
const loading = new Set<string>();
const listeners = new Set<() => void>();
const notify = () => {
  for (const l of listeners) l();
};
const subscribe = (l: () => void) => (listeners.add(l), () => listeners.delete(l));
const countKey = (instanceKey: string, serverId: string) => `${instanceKey}|${serverId}`;

function setCount(instanceKey: string, serverId: string, count: number) {
  const key = countKey(instanceKey, serverId);
  const had = counts.get(key);
  counts.set(key, { count, at: Date.now() });
  if (had?.count !== count) notify();
}

/** Asks how many cases are open (one case's page, for its count). */
export function loadOpenCount(instanceKey: string, serverId: string) {
  const key = countKey(instanceKey, serverId);
  if (loading.has(key)) return;
  loading.add(key);
  listCases(instanceKey, serverId, { status: OPEN, limit: 1 })
    .catch(() => {})
    .finally(() => loading.delete(key));
}

const timers = new Map<string, ReturnType<typeof setTimeout>>();
/** Several changes at once (a burst of reports) ask once. */
function recountSoon(instanceKey: string, serverId: string) {
  const key = countKey(instanceKey, serverId);
  if (!counts.has(key) || timers.has(key)) return;
  timers.set(
    key,
    setTimeout(() => {
      timers.delete(key);
      loadOpenCount(instanceKey, serverId);
    }, 600),
  );
}

/** The count read last, without asking. */
export const openCountNow = (instanceKey: string, serverId: string) => counts.get(countKey(instanceKey, serverId))?.count ?? 0;

/**
 * How many cases are open in a server, for someone who may see them
 * (`enabled`): read when first shown, and again when it's gone stale.
 */
export function useOpenCases(instanceKey: string, serverId: string, enabled: boolean): number {
  const count = useSyncExternalStore(subscribe, () => (enabled ? (counts.get(countKey(instanceKey, serverId))?.count ?? 0) : 0));
  useEffect(() => {
    if (!enabled || !serverId) return;
    const had = counts.get(countKey(instanceKey, serverId));
    if (!had || Date.now() - had.at > STALE_MS) loadOpenCount(instanceKey, serverId);
  }, [enabled, instanceKey, serverId]);
  return count;
}

// ── Events ───────────────────────────────────────────────────────────────

export type ModEvent = NonNullable<Extract<Event["payload"], { case: "modCaseUpdated" | "modNoteAdded" | "modNoteDeleted" }>>;
type Watcher = { instanceKey: string; serverId: string; on: (event: ModEvent) => void };
const watchers = new Set<Watcher>();

/** Hears a server's Mod View events while a screen shows them. */
export function watchMod(instanceKey: string, serverId: string, on: (event: ModEvent) => void): () => void {
  const watcher = { instanceKey, serverId, on };
  watchers.add(watcher);
  return () => void watchers.delete(watcher);
}

/** A server's event: Mod View's go to the screens showing them, and a changed case counts again. */
export function onModEvent(instanceKey: string, event: Event) {
  const p = event.payload;
  if (p.case !== "modCaseUpdated" && p.case !== "modNoteAdded" && p.case !== "modNoteDeleted") return;
  if (p.case === "modCaseUpdated") recountSoon(instanceKey, event.serverId);
  for (const w of watchers) {
    if (w.instanceKey !== instanceKey || w.serverId !== event.serverId) continue;
    try {
      w.on(p);
    } catch {
      // One screen's trouble doesn't keep the event from the others.
    }
  }
}

/** Forgets an instance's counts, when it's removed or signed out of. */
export function forgetModeration(instanceKey: string) {
  for (const key of [...counts.keys()]) if (key.startsWith(`${instanceKey}|`)) counts.delete(key);
  notify();
}

// ── The screen, opened from anywhere ─────────────────────────────────────

/** Where Mod View opens: a tab of the queue, a case, or someone's page. */
export type ModViewPlace = { tab?: string; caseId?: string; userId?: string };
type ModViewOpen = { instanceKey: string; server: Server; place: ModViewPlace; n: number };

let modView: { open: boolean; last: ModViewOpen | null } = { open: false, last: null };
const viewListeners = new Set<() => void>();
const setModView = (next: typeof modView) => {
  modView = next;
  for (const l of viewListeners) l();
};
let opened = 0;

/** Opens Mod View on a server, at a place in it. */
export const openModView = (instanceKey: string, server: Server, place: ModViewPlace = {}) =>
  setModView({ open: true, last: { instanceKey, server, place, n: ++opened } });
export const closeModView = () => setModView({ ...modView, open: false });

/** What Mod View shows: open or not, and the last place it opened at (kept while it closes). */
export const useModView = () =>
  useSyncExternalStore(
    (l) => (viewListeners.add(l), () => viewListeners.delete(l)),
    () => modView,
  );
