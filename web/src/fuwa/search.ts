import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { useSyncExternalStore } from "react";
import { SearchHas, type SearchResult } from "@/gen/fuwa/v1/search_pb";
import { ChannelType, type Channel, type Member, type User } from "@/gen/fuwa/v1/types_pb";
import { i18n } from "@/i18n/i18n";
import { memberName } from "@/lib/format";
import { reportUsage } from "@/lib/reports";
import { hasValue, parseQuery, rememberSearch, timeRange, type Filter } from "@/lib/search-query";
import { engine } from "./sync";
import { store, updateInstance, withUsers } from "./store";
import { toFuwaError } from "./errors";

/**
 * Searching a server's messages (SearchService). One search is open at a
 * time, for the server on screen; its results stay until another search or
 * closing the panel. What was typed is read here into words and filters
 * (`lib/search-query`), with members and channels turned into ids; the
 * instance only ever gets ids, times and words.
 */

export type SearchState = {
  open: boolean;
  instanceKey: string;
  serverId: string;
  /** What was typed for the search on screen. */
  query: string;
  results: SearchResult[];
  total: number;
  /** The count stopped early: there are at least `total`. */
  totalAtLeast: boolean;
  cursor: string;
  loading: boolean;
  /** Loading the next page, not a new search. */
  more: boolean;
  error: string | null;
  indexing: boolean;
  indexedPercent: number;
  /** Bumped by each new search, so the results animate in again. */
  run: number;
};

const CLOSED: SearchState = {
  open: false,
  instanceKey: "",
  serverId: "",
  query: "",
  results: [],
  total: 0,
  totalAtLeast: false,
  cursor: "",
  loading: false,
  more: false,
  error: null,
  indexing: false,
  indexedPercent: 0,
  run: 0,
};

let state = CLOSED;
const listeners = new Set<() => void>();
const set = (patch: Partial<SearchState>) => {
  state = { ...state, ...patch };
  for (const l of listeners) l();
};

export function useSearch<T>(select: (s: SearchState) => T): T {
  return useSyncExternalStore(
    (l) => (listeners.add(l), () => listeners.delete(l)),
    () => select(state),
  );
}

export const getSearch = () => state;

/** Opens the panel without searching (phones open it to type in). */
export function openSearch(instanceKey: string, serverId: string) {
  if (state.open && state.instanceKey === instanceKey && state.serverId === serverId) return;
  set({ ...CLOSED, open: true, instanceKey, serverId, run: state.run });
}

export const closeSearch = () => set({ ...CLOSED, run: state.run });

/** Where recent searches are kept on this device: one list per account and server, so the next person to sign in here doesn't see them. */
export const searchPlace = (instanceKey: string, accountId: string, serverId: string) => `${instanceKey}/${accountId}/${serverId}`;

/** Channels whose messages can be searched. */
export const searchableChannel = (c: Channel) =>
  c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT || c.type === ChannelType.THREAD;

/** The member a `from:` or `mentions:` value names: by username, then by the name shown. */
export function findMember(members: Member[], value: string): Member | undefined {
  const v = value.replace(/^@/, "").toLowerCase();
  return (
    members.find((m) => m.user?.username.toLowerCase() === v) ?? members.find((m) => memberName(m).toLowerCase() === v)
  );
}

/** The channel an `in:` value names. */
export function findChannel(channels: Channel[], value: string): Channel | undefined {
  const v = value.replace(/^#/, "").toLowerCase();
  return channels.find((c) => searchableChannel(c) && c.name.toLowerCase() === v);
}

const HAS: Record<string, SearchHas> = {
  link: SearchHas.LINK,
  embed: SearchHas.EMBED,
  file: SearchHas.FILE,
  picture: SearchHas.PICTURE,
  video: SearchHas.VIDEO,
  sound: SearchHas.SOUND,
  everyone: SearchHas.EVERYONE_MENTION,
};

/** Why a filter can't be used, in words. */
function problem(f: Filter): string {
  const { t } = i18n();
  if (f.key === "from" || f.key === "mentions") return t("system.search.noMember", { name: f.value });
  if (f.key === "in") return t("system.search.noChannel", { channel: f.value.replace(/^#/, "") });
  if (f.key === "has") return t("system.search.hasTakes");
  return t("system.search.dateTakes", { filter: f.key });
}

let inFlight: AbortController | null = null;

/** Searches the server, from the first page. */
export async function runSearch(instanceKey: string, serverId: string, query: string) {
  const s = store.get().instances[instanceKey];
  const members = s?.members[serverId] ?? [];
  const channels = s?.channels[serverId] ?? [];
  const { text, filters } = parseQuery(query);
  const authorIds: string[] = [];
  const mentionIds: string[] = [];
  const channelIds: string[] = [];
  const has: SearchHas[] = [];
  for (const f of filters) {
    const fail = () => set({ open: true, instanceKey, serverId, query, results: [], total: 0, totalAtLeast: false, cursor: "", loading: false, error: problem(f), run: state.run + 1 });
    if (f.key === "from" || f.key === "mentions") {
      const id = findMember(members, f.value)?.user?.id;
      if (!id) return fail();
      (f.key === "from" ? authorIds : mentionIds).push(id);
    } else if (f.key === "in") {
      const id = findChannel(channels, f.value)?.id;
      if (!id) return fail();
      channelIds.push(id);
    } else if (f.key === "has") {
      const value = hasValue(f.value);
      if (!value) return fail();
      has.push(HAS[value]!);
    }
  }
  const range = timeRange(filters, new Date());
  if (range.bad) {
    set({ open: true, instanceKey, serverId, query, results: [], total: 0, totalAtLeast: false, cursor: "", loading: false, error: problem(range.bad), run: state.run + 1 });
    return;
  }
  if (!text.trim() && !filters.length) return;
  const me = s?.me?.id;
  if (me) rememberSearch(searchPlace(instanceKey, me, serverId), query);
  reportUsage("search.run");
  const request = {
    serverId,
    query: text,
    authorIds,
    mentionIds,
    channelIds,
    has,
    after: range.after ? timestampFromDate(range.after) : undefined,
    before: range.before ? timestampFromDate(range.before) : undefined,
  };
  lastRequest = request;
  set({ open: true, instanceKey, serverId, query, results: [], total: 0, totalAtLeast: false, cursor: "", loading: true, more: false, error: null, run: state.run + 1 });
  await fetchPage(instanceKey, "");
}

let lastRequest: Record<string, unknown> | null = null;

/** The next page of the search on screen. */
export async function loadMoreResults() {
  if (!state.open || state.loading || !state.cursor || !lastRequest) return;
  set({ loading: true, more: true });
  await fetchPage(state.instanceKey, state.cursor);
}

async function fetchPage(instanceKey: string, cursor: string) {
  inFlight?.abort();
  const controller = new AbortController();
  inFlight = controller;
  const run = state.run;
  try {
    const res = await engine(instanceKey).api.search.searchMessages({ ...lastRequest, cursor }, { signal: controller.signal });
    if (run !== state.run) return;
    // Authors go in the store like a page of messages', so names and pictures show.
    if (res.authors.length) addAuthors(instanceKey, res.authors);
    set({
      results: cursor ? [...state.results, ...res.results] : res.results,
      total: cursor ? state.total : Number(res.total),
      totalAtLeast: cursor ? state.totalAtLeast : res.totalAtLeast,
      cursor: res.nextCursor,
      loading: false,
      more: false,
      indexing: res.indexing,
      indexedPercent: res.indexedPercent,
    });
  } catch (err) {
    if (controller.signal.aborted || run !== state.run) return;
    set({ loading: false, more: false, error: toFuwaError(err).message });
  } finally {
    if (inFlight === controller) inFlight = null;
  }
}

function addAuthors(instanceKey: string, authors: User[]) {
  updateInstance(instanceKey, (i) => ({ ...i, users: withUsers(i.users, authors) }));
}

// ── Jumping to a result ─────────────────────────────────────────────────────

export type JumpTarget = { instanceKey: string; channelId: string; messageId: string; at: number };

let jump: JumpTarget | null = null;
const jumpListeners = new Set<() => void>();

/** Asks the channel's message list to bring a message into view (it opens the channel first, if needed). */
export function requestJump(instanceKey: string, channelId: string, messageId: string) {
  jump = { instanceKey, channelId, messageId, at: Date.now() };
  for (const l of jumpListeners) l();
}

/** The jump waiting for a channel, if any. */
export function useJump(instanceKey: string, channelId: string): JumpTarget | null {
  return useSyncExternalStore(
    (l) => (jumpListeners.add(l), () => jumpListeners.delete(l)),
    () => (jump && jump.instanceKey === instanceKey && jump.channelId === channelId ? jump : null),
  );
}

/** The message list took the jump. */
export function doneJumping(target: JumpTarget) {
  if (jump === target) jump = null;
}
