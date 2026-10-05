import type { Effect } from "effect";
import { timestampDate } from "@bufbuild/protobuf/wkt";
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Channel, Member, Role } from "@/gen/fuwa/v1/types_pb";
import { accessOf, NO_ACCESS, type Access } from "@/lib/permissions";
import { run } from "./actions";
import type { FuwaError } from "./errors";
import { store, useFuwa, type InstanceState } from "./store";

/**
 * Wraps an action for a button or form: tracks whether it's running and
 * the last error, in words. `go` resolves to the result, or undefined if it failed.
 */
export function useAction<Args extends unknown[], A>(action: (...args: Args) => Effect.Effect<A, FuwaError>) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const latest = useRef(action);
  useLayoutEffect(() => {
    latest.current = action;
  });
  const go = useCallback(async (...args: Args): Promise<A | undefined> => {
    setPending(true);
    setError(null);
    try {
      return await run(latest.current(...args));
    } catch (err) {
      setError((err as FuwaError).message ?? "something went wrong");
      return undefined;
    } finally {
      setPending(false);
    }
  }, []);
  return { go, pending, error, setError };
}

export const useInstance = (key: string | undefined): InstanceState | undefined =>
  useFuwa((s) => (key ? s.instances[key] : undefined));

export const useInstanceOrder = () => useFuwa((s) => s.order);

/** The instances in the order they were added (a stable array while nothing changes). */
let lastOrder: string[] = [];
let lastInstances: InstanceState[] = [];
let lastMap: Record<string, InstanceState> = {};
export function useInstances(): InstanceState[] {
  return useFuwa((s) => {
    if (s.order === lastOrder && s.instances === lastMap) return lastInstances;
    const next = s.order.flatMap((k) => (s.instances[k] ? [s.instances[k]!] : []));
    lastOrder = s.order;
    lastMap = s.instances;
    const same = next.length === lastInstances.length && next.every((i, n) => i === lastInstances[n]);
    if (!same) lastInstances = next;
    return lastInstances;
  });
}

/** What the server rail shows of an instance. */
export type RailInstance = Pick<InstanceState, "key" | "url" | "node" | "connection" | "me" | "servers" | "applied">;

let lastRail: RailInstance[] = [];
const railCache = new Map<string, RailInstance>();
const sameRail = (a: RailInstance, b: InstanceState) =>
  a.url === b.url && a.node === b.node && a.connection === b.connection && a.me === b.me && a.servers === b.servers && a.applied === b.applied;

/**
 * The instances as the rail shows them: the same objects until something the
 * rail draws changes, so a message arriving doesn't redraw the rail.
 */
export function useRailInstances(): RailInstance[] {
  return useFuwa((s) => {
    const next = s.order.flatMap((k) => {
      const i = s.instances[k];
      if (!i) return [];
      const cached = railCache.get(k);
      if (cached && sameRail(cached, i)) return [cached];
      const fresh: RailInstance = { key: i.key, url: i.url, node: i.node, connection: i.connection, me: i.me, servers: i.servers, applied: i.applied };
      railCache.set(k, fresh);
      return [fresh];
    });
    if (next.length !== lastRail.length || next.some((i, n) => i !== lastRail[n])) lastRail = next;
    return lastRail;
  });
}

export const getInstance = (key: string) => store.get().instances[key];

const NO_ROLES: Role[] = [];
const NO_CHANNELS: Channel[] = [];

/** A server's roles, highest first; @everyone last. */
export const useRoles = (key: string, serverId: string): Role[] =>
  useFuwa((s) => s.instances[key]?.roles[serverId] ?? NO_ROLES);

/** Your membership in a server. */
export const useMyMember = (key: string, serverId: string): Member | undefined =>
  useFuwa((s) => {
    const i = s.instances[key];
    return i?.members[serverId]?.find((m) => m.user?.id === i.me?.id);
  });

/** What you can do in a server, worked out as the server does. */
export function useAccess(key: string, serverId: string): Access {
  const ownerId = useFuwa((s) => s.instances[key]?.servers.find((x) => x.id === serverId)?.ownerId);
  const hasRules = useFuwa((s) => !!s.instances[key]?.servers.find((x) => x.id === serverId)?.hasRules);
  const meId = useFuwa((s) => s.instances[key]?.me?.id);
  const member = useMyMember(key, serverId);
  const roles = useRoles(key, serverId);
  const channels = useFuwa((s) => s.instances[key]?.channels[serverId] ?? NO_CHANNELS);
  return useMemo(
    () =>
      ownerId && meId
        ? accessOf(
            serverId,
            ownerId,
            roles,
            channels,
            meId,
            member?.roleIds ?? [],
            !!member?.pending && hasRules,
            // As the server works it out: a timed-out member only reads.
            !!member?.timedOutUntil && timestampDate(member.timedOutUntil).getTime() > Date.now(),
          )
        : NO_ACCESS,
    [serverId, ownerId, meId, member, roles, channels, hasRules],
  );
}

/** What you can do in a server right now, worked out as `useAccess` does, for code that runs outside rendering (a right-click menu). */
export function accessNow(key: string, serverId: string): Access {
  const i = store.get().instances[key];
  const server = i?.servers.find((x) => x.id === serverId);
  const meId = i?.me?.id;
  if (!i || !server?.ownerId || !meId) return NO_ACCESS;
  const member = i.members[serverId]?.find((m) => m.user?.id === meId);
  return accessOf(
    serverId,
    server.ownerId,
    i.roles[serverId] ?? NO_ROLES,
    i.channels[serverId] ?? NO_CHANNELS,
    meId,
    member?.roleIds ?? [],
    !!member?.pending && server.hasRules,
    !!member?.timedOutUntil && timestampDate(member.timedOutUntil).getTime() > Date.now(),
  );
}
