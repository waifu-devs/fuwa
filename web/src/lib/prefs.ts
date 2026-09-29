import { useSyncExternalStore } from "react";
import { applyTheme, BUILTIN_THEMES, isDark, type Theme } from "@/lib/themes";

/**
 * App settings: how fuwa looks and behaves on this device, for every
 * instance at once. They live in this browser's storage (a config file in
 * the desktop app later) and never reach a server. Account settings, which
 * belong to one instance, live on that instance instead.
 */

export type Density = "compact" | "default" | "spacious";
export type MessageDisplay = "cozy" | "compact";
export type ReduceMotion = "system" | "always" | "never";
export type Clock = "auto" | "12h" | "24h";
export type SendWith = "enter" | "mod-enter";
export type NotifyFor = "mentions" | "all";
export type Sound = "message" | "mention" | "join";

export type CustomKeybind = { id: string; action: string; combo: string };

export type Prefs = {
  /** The theme when not following the system. */
  theme: string;
  followSystem: boolean;
  lightTheme: string;
  darkTheme: string;
  density: Density;
  messageDisplay: MessageDisplay;
  /** Message text size, in pixels. */
  chatFontSize: number;
  /** The whole app's size, in percent. */
  zoom: number;
  reduceMotion: ReduceMotion;
  /** Color saturation, in percent. */
  saturation: number;
  underlineLinks: boolean;
  clock: Clock;
  sendWith: SendWith;
  desktopNotifications: boolean;
  notifyFor: NotifyFor;
  unreadBadge: boolean;
  sounds: Record<Sound, boolean>;
  /** Sound volume, in percent. */
  volume: number;
  streamer: boolean;
  streamerHidePersonal: boolean;
  streamerMuteSounds: boolean;
  streamerMuteNotifications: boolean;
  /** Default shortcuts someone changed; null means unbound. */
  keybinds: Record<string, string | null>;
  /** Extra shortcuts someone added, on top of the defaults. */
  customKeybinds: CustomKeybind[];
  developerMode: boolean;
};

const systemDark = () => typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches;

export const DEFAULT_PREFS: Prefs = {
  theme: "sakura",
  followSystem: false,
  lightTheme: "sakura",
  darkTheme: "yoru",
  density: "default",
  messageDisplay: "cozy",
  chatFontSize: 15,
  zoom: 100,
  reduceMotion: "system",
  saturation: 100,
  underlineLinks: false,
  clock: "auto",
  sendWith: "enter",
  desktopNotifications: false,
  notifyFor: "mentions",
  unreadBadge: true,
  sounds: { message: true, mention: true, join: false },
  volume: 60,
  streamer: false,
  streamerHidePersonal: true,
  streamerMuteSounds: true,
  streamerMuteNotifications: true,
  keybinds: {},
  customKeybinds: [],
  developerMode: false,
};

/** The defaults on this device: the theme starts light or dark like the system. */
export const defaultPrefs = (): Prefs => ({ ...DEFAULT_PREFS, theme: systemDark() ? "yoru" : "sakura" });

const KEY = "fuwa:prefs:v1";
/** Where the theme lived before app settings existed. */
const OLD_THEME_KEY = "fuwa:theme";

function load(): Prefs {
  const base = defaultPrefs();
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) {
      const old = localStorage.getItem(OLD_THEME_KEY);
      return old ? { ...base, theme: old } : base;
    }
    const saved = JSON.parse(raw) as Partial<Prefs>;
    return sanitize({ ...base, ...saved, sounds: { ...base.sounds, ...saved.sounds } });
  } catch {
    return base;
  }
}

const clamp = (n: unknown, min: number, max: number, fallback: number) =>
  typeof n === "number" && Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : fallback;
const oneOf = <T extends string>(value: unknown, options: readonly T[], fallback: T): T =>
  options.includes(value as T) ? (value as T) : fallback;
const themeId = (id: unknown, fallback: string) => (BUILTIN_THEMES.some((t) => t.id === id) ? (id as string) : fallback);

/** Stored values from an older or hand-edited version fall back to defaults instead of breaking the app. */
function sanitize(p: Prefs): Prefs {
  const d = DEFAULT_PREFS;
  return {
    ...p,
    theme: themeId(p.theme, "sakura"),
    lightTheme: themeId(p.lightTheme, d.lightTheme),
    darkTheme: themeId(p.darkTheme, d.darkTheme),
    density: oneOf(p.density, ["compact", "default", "spacious"], d.density),
    messageDisplay: oneOf(p.messageDisplay, ["cozy", "compact"], d.messageDisplay),
    chatFontSize: clamp(p.chatFontSize, 12, 20, d.chatFontSize),
    zoom: clamp(p.zoom, 80, 150, d.zoom),
    reduceMotion: oneOf(p.reduceMotion, ["system", "always", "never"], d.reduceMotion),
    saturation: clamp(p.saturation, 0, 100, d.saturation),
    clock: oneOf(p.clock, ["auto", "12h", "24h"], d.clock),
    sendWith: oneOf(p.sendWith, ["enter", "mod-enter"], d.sendWith),
    notifyFor: oneOf(p.notifyFor, ["mentions", "all"], d.notifyFor),
    volume: clamp(p.volume, 0, 100, d.volume),
    keybinds: p.keybinds && typeof p.keybinds === "object" ? p.keybinds : {},
    customKeybinds: Array.isArray(p.customKeybinds)
      ? p.customKeybinds.filter((k) => typeof k?.id === "string" && typeof k.action === "string" && typeof k.combo === "string")
      : [],
  };
}

let prefs: Prefs = load();
const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

export const getPrefs = () => prefs;

/** Changes some settings, applies them to the page and remembers them on this device. */
export function setPrefs(patch: Partial<Prefs> | ((p: Prefs) => Partial<Prefs>)) {
  const changes = typeof patch === "function" ? patch(prefs) : patch;
  prefs = sanitize({ ...prefs, ...changes });
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    // Applied for now, not remembered.
  }
  applyPrefs();
  emit();
}

export function subscribePrefs(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Reads one app setting (or something made from them) and re-renders when it changes. */
export function usePrefs<T>(select: (p: Prefs) => T): T {
  return useSyncExternalStore(subscribePrefs, () => select(prefs));
}

const themeById = (id: string): Theme => BUILTIN_THEMES.find((t) => t.id === id) ?? BUILTIN_THEMES[0]!;

/** The theme on screen now: the picked one, or the light or dark pick when following the system. */
export function activeTheme(p: Prefs = prefs): Theme {
  if (!p.followSystem) return themeById(p.theme);
  return themeById(systemDark() ? p.darkTheme : p.lightTheme);
}

export const LIGHT_THEMES = BUILTIN_THEMES.filter((t) => !isDark(t));
export const DARK_THEMES = BUILTIN_THEMES.filter((t) => isDark(t));

/** Whether motion should calm down, from the setting or, by default, the system. */
export function reduceMotion(p: Prefs = prefs): boolean {
  if (p.reduceMotion === "always") return true;
  if (p.reduceMotion === "never") return false;
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** Puts the app settings on the page: theme, sizes, spacing and the accessibility switches. */
export function applyPrefs() {
  if (typeof document === "undefined") return;
  const p = prefs;
  applyTheme(activeTheme(p));
  const root = document.documentElement;
  // Everything is sized in rem, so the root size zooms the whole app.
  root.style.fontSize = p.zoom === 100 ? "" : `${(16 * p.zoom) / 100}px`;
  root.style.setProperty("--chat-font", `${p.chatFontSize}px`);
  root.dataset.density = p.density;
  root.dataset.display = p.messageDisplay;
  root.style.setProperty("--saturation", `${p.saturation}%`);
  toggleData(root, "desaturated", p.saturation < 100);
  toggleData(root, "underlineLinks", p.underlineLinks);
  toggleData(root, "reduceMotion", reduceMotion(p));
}

function toggleData(el: HTMLElement, key: string, on: boolean) {
  if (on) el.dataset[key] = "";
  else delete el.dataset[key];
}

/** Follows the system's light or dark mode and motion setting, and settings changed in another tab. */
export function watchSystem() {
  const dark = window.matchMedia("(prefers-color-scheme: dark)");
  const motion = window.matchMedia("(prefers-reduced-motion: reduce)");
  const again = () => {
    applyPrefs();
    emit();
  };
  dark.addEventListener("change", again);
  motion.addEventListener("change", again);
  window.addEventListener("storage", (e) => {
    if (e.key !== KEY) return;
    prefs = load();
    again();
  });
}
