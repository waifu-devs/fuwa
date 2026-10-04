import { useSyncExternalStore } from "react";

/**
 * Which rail folders are open, on this device only (opening one on your
 * phone shouldn't open it on your desktop). Keyed by instance and folder.
 */

const KEY = "fuwa.rail.open";

function load(): Set<string> {
  try {
    const raw = localStorage.getItem(KEY);
    const list: unknown = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(list) ? list.filter((x): x is string => typeof x === "string").slice(0, 500) : []);
  } catch {
    return new Set();
  }
}

let open = load();
let version = 0;
const listeners = new Set<() => void>();
const subscribe = (l: () => void) => (listeners.add(l), () => listeners.delete(l));

const at = (instance: string, folder: string) => `${instance}/${folder}`;

export const isFolderOpen = (instance: string, folder: string) => open.has(at(instance, folder));

export function setFolderOpen(instance: string, folder: string, value: boolean) {
  if (isFolderOpen(instance, folder) === value) return;
  open = new Set(open);
  if (value) open.add(at(instance, folder));
  else open.delete(at(instance, folder));
  version++;
  try {
    localStorage.setItem(KEY, JSON.stringify([...open]));
  } catch {
    // Private windows: open folders just aren't remembered.
  }
  for (const l of listeners) l();
}

export const useFolderOpen = (instance: string, folder: string) => useSyncExternalStore(subscribe, () => isFolderOpen(instance, folder));

/** Changes whenever any folder opens or closes, for what has to move with them. */
export const useFoldersVersion = () => useSyncExternalStore(subscribe, () => version);
