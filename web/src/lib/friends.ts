import type { Friend, FriendEvent, FriendSettings } from "@/gen/fuwa/v1/friend_pb";
import type { I18n } from "../i18n/i18n.ts";

/*
 * Friends, requests and blocks on one instance (docs/friends.md): what the
 * store keeps and the small decisions the screens share. Only type imports,
 * so node's test runner reads this file as is; the FriendState numbers are
 * the protocol's.
 */

/** fuwa.v1.FriendState. */
export const FRIEND = 1;
export const OUTGOING = 2;
export const INCOMING = 3;
export const BLOCKED = 4;

export type FriendsState = {
  /** loading: the first list is on its way. unsupported: an instance from before friends. */
  status: "off" | "loading" | "ready" | "unsupported";
  /** Everyone in your list, by name. */
  list: Friend[];
  /** Your settings, once loaded. */
  settings: FriendSettings | null;
};

export const emptyFriends = (): FriendsState => ({ status: "off", list: [], settings: null });

export type FriendsTab = "online" | "all" | "pending" | "blocked";

const nameOf = (f: Friend) => (f.user?.displayName || f.user?.username || "").toLocaleLowerCase();

/** By name, then id, so the list doesn't shuffle. */
export function sortFriends(list: Friend[]): Friend[] {
  return [...list].sort((a, b) => nameOf(a).localeCompare(nameOf(b)) || (a.user?.id ?? "").localeCompare(b.user?.id ?? ""));
}

const millis = (ts: Friend["expiresAt"]) => (ts ? Number(ts.seconds) * 1000 + Math.floor(ts.nanos / 1e6) : null);

/** A request that ran out is as good as gone; the instance sweeps it soon. */
export const expired = (f: Friend, now = Date.now()) => {
  const at = millis(f.expiresAt);
  return at !== null && at <= now;
};

/** Your list after an event from the instance. Events can arrive twice, so each one is idempotent. */
export function applyFriendEvent(list: Friend[], event: FriendEvent): Friend[] {
  const p = event.payload;
  switch (p.case) {
    case "changed": {
      const id = p.value.user?.id;
      if (!id) return list;
      return sortFriends([...list.filter((f) => f.user?.id !== id), p.value]);
    }
    case "removed":
      return list.some((f) => f.user?.id === p.value) ? list.filter((f) => f.user?.id !== p.value) : list;
    case "presence": {
      const { userId, online } = p.value;
      let changed = false;
      const next = list.map((f) => {
        if (f.user?.id !== userId || f.state !== FRIEND || f.online === online) return f;
        changed = true;
        return { ...f, online };
      });
      return changed ? next : list;
    }
    default:
      return list;
  }
}

/** Who goes under each tab, filtered by a search. */
export function inTab(list: Friend[], tab: FriendsTab, query = "", now = Date.now()): Friend[] {
  const q = query.trim().toLocaleLowerCase().replace(/^@/, "");
  return list.filter((f) => {
    if (expired(f, now)) return false;
    const fits =
      tab === "online" ? f.state === FRIEND && f.online
      : tab === "all" ? f.state === FRIEND
      : tab === "pending" ? f.state === INCOMING || f.state === OUTGOING
      : f.state === BLOCKED;
    if (!fits) return false;
    if (!q) return true;
    return nameOf(f).includes(q) || (f.user?.username ?? "").includes(q);
  });
}

/** Requests waiting for your answer: the badge. */
export const waitingForYou = (list: Friend[], now = Date.now()) => list.filter((f) => f.state === INCOMING && !expired(f, now)).length;

/** Where you stand with someone, from your list. 0 when there's nothing. */
export function stateWith(list: Friend[], userId: string | undefined, now = Date.now()): number {
  const f = list.find((x) => x.user?.id === userId);
  return f && !expired(f, now) ? f.state : 0;
}

/** Ids of people you blocked, whose conversations stay out of sight. */
export const blockedIds = (list: Friend[]) => new Set(list.filter((f) => f.state === BLOCKED).map((f) => f.user?.id ?? ""));

/** A username as typed into "Add friend": a leading @ and spaces don't count. */
export const cleanUsername = (typed: string) => typed.trim().replace(/^@+/, "").toLowerCase();

/** What a pending request's line says, in the app's language. */
export function pendingLine(t: I18n["t"], f: Friend, now = Date.now()): string {
  const at = millis(f.expiresAt);
  const count = at === null ? null : Math.max(1, Math.ceil((at - now) / 86_400_000));
  if (f.state === INCOMING) return count === null ? t("dms-calls.friends.pending.incoming") : t("dms-calls.friends.pending.incomingLeft", { count });
  return count === null ? t("dms-calls.friends.pending.outgoing") : t("dms-calls.friends.pending.outgoingLeft", { count });
}
