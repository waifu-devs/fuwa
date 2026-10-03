import { useSyncExternalStore } from "react";

/**
 * What happened when fuwa last ran each custom shader on this device, by
 * shaderId: whether it ran, at what resolution, or why it didn't. The
 * backdrop writes it, the shader editor reads it.
 *
 * A shader that stopped the GPU (or the whole tab) is remembered across
 * reloads: before a shader's first frames fuwa notes it as "trying" in
 * localStorage, and clears the note once the frames came back in time. A
 * note still there on the next run means it never did, so it isn't run
 * again until someone changes it or asks to try again.
 */

export type ShaderStatus =
  | { state: "running"; scale: number; ms: number }
  | { state: "broken"; message: string; line: number | null }
  | { state: "slow"; ms: number }
  | { state: "stopped" };

const KEY = "fuwa:shaders-trying";
const statuses = new Map<string, ShaderStatus>();
const listeners = new Set<() => void>();

function trying(): string[] {
  try {
    const list = JSON.parse(localStorage.getItem(KEY) ?? "[]") as unknown;
    return Array.isArray(list) ? list.filter((id): id is string => typeof id === "string").slice(-20) : [];
  } catch {
    return [];
  }
}

function saveTrying(list: string[]) {
  try {
    if (list.length) localStorage.setItem(KEY, JSON.stringify(list.slice(-20)));
    else localStorage.removeItem(KEY);
  } catch {
    // Storage off: a shader that stops the GPU is only remembered until a reload.
  }
}

// Shaders whose first frames never came back last time (this page's own are only added once they fail).
for (const id of typeof localStorage === "undefined" ? [] : trying()) statuses.set(id, { state: "stopped" });

export function shaderStatus(id: string): ShaderStatus | undefined {
  return statuses.get(id);
}

export function setShaderStatus(id: string, status: ShaderStatus | null) {
  if (status) statuses.set(id, status);
  else statuses.delete(id);
  for (const listener of listeners) listener();
}

/** Marks a shader as about to run its first frames; `done` says they came back (or it was let go before they did). */
export function startTrying(id: string): (ok: boolean) => void {
  saveTrying([...trying().filter((t) => t !== id), id]);
  return (ok) => {
    if (ok) saveTrying(trying().filter((t) => t !== id));
  };
}

/** Forgets what happened to a shader, so it's tried again. */
export function retryShader(id: string) {
  saveTrying(trying().filter((t) => t !== id));
  setShaderStatus(id, null);
}

export function useShaderStatus(id: string | null): ShaderStatus | undefined {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => (id ? shaderStatus(id) : undefined),
    () => undefined,
  );
}
