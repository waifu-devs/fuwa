import { Code } from "@connectrpc/connect";
import { create } from "@bufbuild/protobuf";
import { useSyncExternalStore } from "react";
import {
  PresenceSettingsSchema,
  PresenceStatus,
  type Presence,
  type PresenceSettings,
} from "@/gen/fuwa/v1/presence_pb";
import { reportError, reportUsage } from "@/lib/reports";
import type { Api } from "./client";
import { toFuwaError } from "./errors";

/**
 * Who's online and what they're doing (docs/presence.md), per instance.
 *
 * Kept apart from the main store on purpose: presence changes often, and
 * each dot or activity line subscribes to one person, so a change redraws
 * only what shows that person, never the member list or the chat.
 */

/** No input for this long and the app says you're away. */
const IDLE_AFTER = 10 * 60_000;
/** How long a burst of status changes waits before the member list regroups. */
const REGROUP = 400;

type Instance = {
  people: Map<string, Presence>;
  /** The instance answers PresenceService (older ones don't). */
  supported: boolean;
  settings: PresenceSettings | null;
  /** Who's online, rebuilt at most every `REGROUP` ms; a new Set each time. */
  online: Set<string>;
  regroup: ReturnType<typeof setTimeout> | null;
};

const instances = new Map<string, Instance>();
const personListeners = new Map<string, Set<() => void>>();
const instanceListeners = new Map<string, Set<() => void>>();
const NO_ONE = new Set<string>();

const instance = (key: string) => {
  let i = instances.get(key);
  if (!i) {
    i = { people: new Map(), supported: false, settings: null, online: NO_ONE, regroup: null };
    instances.set(key, i);
  }
  return i;
};

const personKey = (key: string, userId: string) => `${key}\n${userId}`;

const listen = (map: Map<string, Set<() => void>>, id: string, fn: () => void) => {
  let set = map.get(id);
  if (!set) map.set(id, (set = new Set()));
  set.add(fn);
  return () => {
    set.delete(fn);
    if (!set.size) map.delete(id);
  };
};

const notify = (map: Map<string, Set<() => void>>, id: string) => {
  for (const fn of map.get(id) ?? []) fn();
};

const isOnline = (p: Presence | undefined) => !!p && p.status !== PresenceStatus.OFFLINE && p.status !== PresenceStatus.UNSPECIFIED;

/** Regroups the member list soon, once for a burst of changes. */
function scheduleRegroup(key: string) {
  const i = instance(key);
  if (i.regroup) return;
  i.regroup = setTimeout(() => {
    i.regroup = null;
    i.online = new Set([...i.people.values()].filter(isOnline).map((p) => p.userId));
    notify(instanceListeners, key);
  }, REGROUP);
}

function put(key: string, presence: Presence) {
  const i = instance(key);
  const before = i.people.get(presence.userId);
  if (isOnline(presence)) i.people.set(presence.userId, presence);
  else i.people.delete(presence.userId);
  notify(personListeners, personKey(key, presence.userId));
  if (isOnline(before) !== isOnline(presence)) scheduleRegroup(key);
}

/** Replaces everyone at once, after a stream (re)starts. */
function replaceAll(key: string, list: Presence[]) {
  const i = instance(key);
  const old = i.people;
  i.people = new Map(list.filter(isOnline).map((p) => [p.userId, p]));
  for (const id of new Set([...old.keys(), ...i.people.keys()])) {
    if (old.get(id) !== i.people.get(id)) notify(personListeners, personKey(key, id));
  }
  scheduleRegroup(key);
}

function forget(key: string) {
  const i = instances.get(key);
  if (!i) return;
  if (i.regroup) clearTimeout(i.regroup);
  instances.delete(key);
  for (const id of i.people.keys()) notify(personListeners, personKey(key, id));
  notify(instanceListeners, key);
}

/** Someone's presence on an instance; undefined while they're offline. */
export function usePresence(key: string | undefined, userId: string | undefined): Presence | undefined {
  return useSyncExternalStore(
    (fn) => (key && userId ? listen(personListeners, personKey(key, userId), fn) : () => {}),
    () => (key && userId ? instances.get(key)?.people.get(userId) : undefined),
  );
}

/** Who's online, or null when the instance doesn't keep presence. Changes at most a few times a second. */
export function useOnline(key: string): Set<string> | null {
  return useSyncExternalStore(
    (fn) => listen(instanceListeners, key, fn),
    () => {
      const i = instances.get(key);
      return i?.supported ? i.online : null;
    },
  );
}

/** Your presence settings, once loaded; null on instances without presence. */
export function usePresenceSettings(key: string): PresenceSettings | null {
  return useSyncExternalStore(
    (fn) => listen(instanceListeners, key, fn),
    () => instances.get(key)?.settings ?? null,
  );
}

export const presenceSettings = (key: string) => instances.get(key)?.settings ?? null;

// ─────────────── Idle ───────────────

let lastInput = Date.now();
let idle = false;
const idleListeners = new Set<() => void>();
let watchingInput = false;

function watchInput() {
  if (watchingInput || typeof window === "undefined") return;
  watchingInput = true;
  const seen = () => {
    lastInput = Date.now();
    if (idle) {
      idle = false;
      for (const fn of idleListeners) fn();
    }
  };
  for (const name of ["pointerdown", "keydown", "wheel", "touchstart", "focus"] as const) {
    window.addEventListener(name, seen, { passive: true, capture: true });
  }
  // Moving the pointer counts too, looked at at most every few seconds.
  let moved = 0;
  window.addEventListener(
    "pointermove",
    () => {
      const now = Date.now();
      if (now - moved > 5000) {
        moved = now;
        seen();
      }
    },
    { passive: true },
  );
  setInterval(() => {
    if (!idle && Date.now() - lastInput > IDLE_AFTER) {
      idle = true;
      for (const fn of idleListeners) fn();
    }
  }, 30_000);
}

// ─────────────── Following an instance ───────────────

const sleep = (ms: number, signal: AbortSignal) =>
  new Promise<void>((resolve) => {
    const t = setTimeout(resolve, ms);
    signal.addEventListener("abort", () => (clearTimeout(t), resolve()), { once: true });
  });

/**
 * Keeps this app's presence on an instance (every minute, and at once when
 * you go idle or come back) and follows everyone you can see. Returns a stop
 * function. An instance without presence is left alone.
 */
export function startPresence(key: string, api: Api): () => void {
  const abort = new AbortController();
  const { signal } = abort;
  watchInput();
  const i = instance(key);

  let renew = 60;
  let wake: (() => void) | null = null;
  const onIdle = () => wake?.();
  idleListeners.add(onIdle);

  const heartbeat = async () => {
    while (!signal.aborted) {
      try {
        const res = await api.presence.updatePresence({ app: "web", idle, activities: [] }, { signal });
        renew = Math.max(15, Math.min(res.renewSeconds || 60, 120));
      } catch (err) {
        if (signal.aborted) return;
        const e = toFuwaError(err);
        if (e.code === Code.Unimplemented) return;
        if (!e.retryable) reportError("presence_update", "presence");
        renew = 15;
      }
      await new Promise<void>((resolve) => {
        const t = setTimeout(resolve, renew * 1000);
        wake = () => (clearTimeout(t), resolve());
        signal.addEventListener("abort", () => (clearTimeout(t), resolve()), { once: true });
      });
      wake = null;
    }
  };

  const watch = async () => {
    let delay = 500;
    while (!signal.aborted) {
      const gathered: Presence[] = [];
      let ready = false;
      try {
        for await (const res of api.presence.watchPresence({}, { signal })) {
          if (res.presence) {
            if (ready) put(key, res.presence);
            else gathered.push(res.presence);
          }
          if (res.ready && !ready) {
            ready = true;
            delay = 500;
            replaceAll(key, gathered);
          }
        }
      } catch (err) {
        if (signal.aborted) return;
        const e = toFuwaError(err);
        if (e.code === Code.Unimplemented || e.signedOut) return;
        if (!e.retryable) reportError("presence_watch", "presence");
      }
      await sleep(delay + Math.random() * delay, signal);
      delay = Math.min(delay * 2, 20_000);
    }
  };

  void (async () => {
    try {
      const { settings } = await api.presence.getPresenceSettings({}, { signal });
      i.settings = settings ?? create(PresenceSettingsSchema, { status: PresenceStatus.ONLINE });
      i.supported = true;
      notify(instanceListeners, key);
    } catch (err) {
      // An instance from before presence: nothing to do here.
      if (!signal.aborted && toFuwaError(err).code !== Code.Unimplemented) i.supported = true;
      if (!i.supported) return;
    }
    void heartbeat();
    void watch();
  })();

  return () => {
    abort.abort();
    idleListeners.delete(onIdle);
    forget(key);
  };
}

/** Saves new presence settings (status, sharing), showing them at once. */
export async function savePresenceSettings(key: string, api: Api, change: (s: PresenceSettings) => PresenceSettings) {
  const i = instance(key);
  const before = i.settings ?? create(PresenceSettingsSchema, { status: PresenceStatus.ONLINE });
  const next = change(before);
  i.settings = next;
  notify(instanceListeners, key);
  try {
    const { settings } = await api.presence.updatePresenceSettings({ settings: next });
    if (settings) i.settings = settings;
    notify(instanceListeners, key);
    if (next.status !== before.status) reportUsage(`presence.status_${PresenceStatus[next.status]?.toLowerCase() ?? "other"}`);
    if (next.showActivity !== before.showActivity) reportUsage(next.showActivity ? "presence.share_on" : "presence.share_off");
  } catch (err) {
    i.settings = before;
    notify(instanceListeners, key);
    throw toFuwaError(err);
  }
}

/** You picked do not disturb on this instance: no sounds or notifications from it. */
export const doNotDisturb = (key: string) => instances.get(key)?.settings?.status === PresenceStatus.DO_NOT_DISTURB;
