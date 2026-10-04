import { useSyncExternalStore } from "react";
import { reportUsage } from "@/lib/reports";

/**
 * Screens anything in the app can open: settings, the shortcut overlay, the
 * quick switcher; plus short notes that pop up at the bottom ("Copied").
 */

export type Toast = { id: number; text: string };

type Ui = {
  /** The section settings are open at, or null when closed. */
  settings: string | null;
  /** What the section opened for, such as a server's id on the notifications page. */
  settingsTarget: string | null;
  shortcuts: boolean;
  switcher: boolean;
  /** Streamer mode's banner, hidden for this visit. */
  streamerBannerHidden: boolean;
  toasts: Toast[];
};

let ui: Ui = { settings: null, settingsTarget: null, shortcuts: false, switcher: false, streamerBannerHidden: false, toasts: [] };
const listeners = new Set<() => void>();

function set(patch: Partial<Ui>) {
  ui = { ...ui, ...patch };
  for (const l of listeners) l();
}

export function useUi<T>(select: (u: Ui) => T): T {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => select(ui),
  );
}

export const getUi = () => ui;

let lastSection = "appearance";

/** Opens settings at a section (for one thing on it, like a server), or where they were last. */
export function openSettings(section?: string, target: string | null = null) {
  if (section) lastSection = section;
  if (ui.settings === null) reportUsage("settings.open");
  set({ settings: section ?? lastSection, settingsTarget: target, shortcuts: false, switcher: false });
}

export function setSettingsSection(section: string) {
  lastSection = section;
  if (ui.settings !== null) set({ settings: section });
}

export const closeSettings = () => set({ settings: null });
export const setShortcuts = (open: boolean) => set({ shortcuts: open, switcher: open ? false : ui.switcher });
export const setSwitcher = (open: boolean) => {
  if (open && !ui.switcher) reportUsage("quick_switcher.open");
  set({ switcher: open, shortcuts: open ? false : ui.shortcuts });
};
export const hideStreamerBanner = (hidden = true) => set({ streamerBannerHidden: hidden });

let nextToast = 1;

/** A short note at the bottom of the screen, gone after a moment. */
export function toast(text: string) {
  const id = nextToast++;
  set({ toasts: [...ui.toasts.slice(-2), { id, text }] });
  setTimeout(() => set({ toasts: ui.toasts.filter((t) => t.id !== id) }), 2200);
}

/** Copies text and says so. */
export function copy(text: string, what: string) {
  void navigator.clipboard?.writeText(text).then(
    () => toast(`Copied ${what}`),
    () => toast(`Couldn't copy ${what}`),
  );
}

const commands = new Map<string, (arg?: string) => void>();

/** Lets the part of the app that owns something handle a command for it from anywhere, like the members list. */
export function onCommand(name: string, run: (arg?: string) => void) {
  commands.set(name, run);
  return () => {
    if (commands.get(name) === run) commands.delete(name);
  };
}

/** Runs a command if something on screen handles it; false otherwise. */
export function runCommand(name: string, arg?: string) {
  const run = commands.get(name);
  run?.(arg);
  return !!run;
}

/** Whether something on screen handles a command right now. */
export const hasCommand = (name: string) => commands.has(name);
