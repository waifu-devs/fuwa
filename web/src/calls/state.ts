import { useSyncExternalStore } from "react";

/**
 * The call this browser is in (one at a time, on any instance), and how you
 * sound in it. Who else is in which call comes from the instances
 * themselves (`InstanceState.voice` and `DmState.calls`); this is only what
 * this browser's own connection is doing.
 */

export type CallTarget =
  | { kind: "voice"; instance: string; serverId: string; channelId: string }
  | { kind: "dm"; instance: string; conversationId: string };

/**
 * connecting: joining for the first time. connected: sound goes both ways.
 * reconnecting: the connection dropped or the media server restarted, and
 * it's joining again with the same place in the call.
 */
export type CallStatus = "connecting" | "connected" | "reconnecting";

export type ActiveCall = {
  target: CallTarget;
  status: CallStatus;
  /** When sound first went through, in ms since the epoch. */
  since: number | null;
  /** Something worth saying about the call, such as the microphone being blocked. */
  problem: string | null;
};

export type CallsState = {
  call: ActiveCall | null;
  /** Muted and deafened stay as you left them, from one call to the next, as Discord does. */
  selfMute: boolean;
  selfDeaf: boolean;
  /** Your camera is on. Unlike mute, every call starts with it off. */
  selfVideo: boolean;
  /** You're sharing your screen. Every call starts without. */
  selfStream: boolean;
  /** Who's talking right now in your call, by user id (you included). */
  speaking: Record<string, boolean>;
  /** Push to talk's key is down. */
  talking: boolean;
  /** Calls you turned down, by "instance/conversation/started at", so they stop ringing. */
  declined: Record<string, true>;
  /** Why the last call ended, when it wasn't you hanging up. */
  ended: string | null;
};

const SAVED = "fuwa:calls:v1";

function load(): Pick<CallsState, "selfMute" | "selfDeaf"> {
  try {
    const saved = JSON.parse(localStorage.getItem(SAVED) ?? "{}") as Partial<CallsState>;
    return { selfMute: saved.selfMute === true, selfDeaf: saved.selfDeaf === true };
  } catch {
    return { selfMute: false, selfDeaf: false };
  }
}

let state: CallsState = { call: null, speaking: {}, talking: false, declined: {}, ended: null, selfVideo: false, selfStream: false, ...load() };
const listeners = new Set<() => void>();

export const getCalls = () => state;

export function setCalls(fn: (s: CallsState) => Partial<CallsState>) {
  const patch = fn(state);
  const next = { ...state, ...patch };
  if (Object.keys(patch).every((k) => next[k as keyof CallsState] === state[k as keyof CallsState])) return;
  const saved = next.selfMute !== state.selfMute || next.selfDeaf !== state.selfDeaf;
  state = next;
  if (saved) {
    try {
      localStorage.setItem(SAVED, JSON.stringify({ selfMute: state.selfMute, selfDeaf: state.selfDeaf }));
    } catch {
      // Kept for this visit only.
    }
  }
  for (const l of listeners) l();
}

export function subscribeCalls(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useCalls<T>(select: (s: CallsState) => T): T {
  return useSyncExternalStore(subscribeCalls, () => select(state));
}

export const sameTarget = (a: CallTarget | null | undefined, b: CallTarget | null | undefined) =>
  !!a &&
  !!b &&
  a.instance === b.instance &&
  (a.kind === "voice" && b.kind === "voice"
    ? a.serverId === b.serverId && a.channelId === b.channelId
    : a.kind === "dm" && b.kind === "dm" && a.conversationId === b.conversationId);

/** Whether you're in this voice channel, here in this browser. */
export const useInVoice = (instance: string, channelId: string) =>
  useCalls((s) => s.call?.target.kind === "voice" && s.call.target.instance === instance && s.call.target.channelId === channelId);

/** Whether you're in this conversation's call, here in this browser. */
export const useInDmCall = (instance: string, conversationId: string) =>
  useCalls((s) => s.call?.target.kind === "dm" && s.call.target.instance === instance && s.call.target.conversationId === conversationId);

export const declineKey = (instance: string, conversationId: string, startedAt: bigint | number) =>
  `${instance}/${conversationId}/${startedAt}`;
