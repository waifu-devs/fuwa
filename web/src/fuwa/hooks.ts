import type { Effect } from "effect";
import { useCallback, useMemo, useRef, useState } from "react";
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
  latest.current = action;
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
        ? accessOf(serverId, ownerId, roles, channels, meId, member?.roleIds ?? [], !!member?.pending && hasRules)
        : NO_ACCESS,
    [serverId, ownerId, meId, member, roles, channels, hasRules],
  );
}
