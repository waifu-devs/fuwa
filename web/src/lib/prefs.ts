import { useSyncExternalStore } from "react";
import { DEFAULT_BACKDROP, sanitizeBackdrop, type Backdrop } from "@/lib/backdrop";
import { MAX_CUSTOM_THEMES, sanitizeCustomTheme, type CustomTheme } from "@/lib/theme-file";
import { isTag } from "@/i18n/core";
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
export type Sound = "message" | "mention" | "join" | "call" | "ring";
/** How your microphone decides when you're talking. */
export type InputMode = "voice" | "ptt";
/** Popped-out cameras: fill the window (cropping the edges) or fit in it whole. */
export type PopoutFit = "cover" | "contain";
/** Where role colors show: on names, as a dot beside them, or not at all. */
export type RoleColors = "names" | "beside" | "off";

export type CustomKeybind = { id: string; action: string; combo: string };

export type Prefs = {
  /** The theme when not following the system. */
  theme: string;
  followSystem: boolean;
  lightTheme: string;
  darkTheme: string;
  /** Themes made, edited or imported on this device. */
  customThemes: CustomTheme[];
  /** The picture and effect behind the app, under themes that don't bring their own. */
  backdrop: Backdrop;
  density: Density;
  messageDisplay: MessageDisplay;
  /** Message text size, in pixels. */
  chatFontSize: number;
  /** The whole app's size, in percent. */
  zoom: number;
  reduceMotion: ReduceMotion;
  /** Effects on other people's profile cards play (your own always shows to you). */
  othersEffects: boolean;
  /** Color saturation, in percent. */
  saturation: number;
  underlineLinks: boolean;
  roleColors: RoleColors;
  /** The app's language: "auto" follows the browser's, or a language tag from locales/. */
  language: string;
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
  /** Anonymous reports of errors and slow paths, sent to your instance (see lib/reports.ts). */
  shareReports: boolean;
  /** Voice and audio. Device ids are this browser's; "" is the system default. */
  inputDevice: string;
  outputDevice: string;
  /** Microphone and call volume, in percent (up to 200). */
  inputVolume: number;
  outputVolume: number;
  inputMode: InputMode;
  /** Voice activity: pick the level by itself, or use `sensitivity` (in dB). */
  autoSensitivity: boolean;
  sensitivity: number;
  /** How long push to talk keeps going after the key comes up, in milliseconds. */
  pttRelease: number;
  echoCancellation: boolean;
  noiseSuppression: boolean;
  autoGainControl: boolean;
  /** How loud each person is for you, in percent, by "instance/user id". Missing means 100. */
  userVolumes: Record<string, number>;
  /** The camera, this browser's device id; "" is the system default. */
  videoDevice: string;
  /** Your own camera shows mirrored to you, as a mirror would (others always see it the right way round). */
  mirrorVideo: boolean;
  /** Popped-out cameras: the name under the picture, a glow while they talk, and filling the window or fitting in it. */
  popoutName: boolean;
  popoutGlow: boolean;
  popoutFit: PopoutFit;
  /** Sharing a screen brings its sound too, where the browser can. */
  shareSound: boolean;
};

const systemDark = () => typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches;

export const DEFAULT_PREFS: Prefs = {
  theme: "sakura",
  followSystem: false,
  lightTheme: "sakura",
  darkTheme: "yoru",
  customThemes: [],
  backdrop: DEFAULT_BACKDROP,
  density: "default",
  messageDisplay: "cozy",
  chatFontSize: 15,
  zoom: 100,
  reduceMotion: "system",
  othersEffects: true,
  saturation: 100,
  underlineLinks: false,
  roleColors: "names",
  language: "auto",
  clock: "auto",
  sendWith: "enter",
  desktopNotifications: false,
  notifyFor: "mentions",
  unreadBadge: true,
  sounds: { message: true, mention: true, join: false, call: true, ring: true },
  volume: 60,
  streamer: false,
  streamerHidePersonal: true,
  streamerMuteSounds: true,
  streamerMuteNotifications: true,
  keybinds: {},
  customKeybinds: [],
  developerMode: false,
  shareReports: true,
  inputDevice: "",
  outputDevice: "",
  inputVolume: 100,
  outputVolume: 100,
  inputMode: "voice",
  autoSensitivity: true,
  sensitivity: -50,
  pttRelease: 200,
  echoCancellation: true,
  noiseSuppression: true,
  autoGainControl: true,
  userVolumes: {},
  videoDevice: "",
  mirrorVideo: true,
  popoutName: true,
  popoutGlow: true,
  popoutFit: "cover",
  shareSound: true,
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

/** Stored values from an older or hand-edited version fall back to defaults instead of breaking the app. */
function sanitize(p: Prefs): Prefs {
  const d = DEFAULT_PREFS;
  const customThemes = (Array.isArray(p.customThemes) ? p.customThemes : [])
    .map(sanitizeCustomTheme)
    .filter((t, n, all): t is CustomTheme => !!t && all.findIndex((o) => o?.id === t.id) === n)
    .slice(0, MAX_CUSTOM_THEMES);
  const known = (id: unknown) => BUILTIN_THEMES.some((t) => t.id === id) || customThemes.some((t) => t.id === id);
  const themeId = (id: unknown, fallback: string) => (known(id) ? (id as string) : fallback);
  return {
    ...p,
    customThemes,
    backdrop: sanitizeBackdrop(p.backdrop),
    theme: themeId(p.theme, "sakura"),
    lightTheme: themeId(p.lightTheme, d.lightTheme),
    darkTheme: themeId(p.darkTheme, d.darkTheme),
    density: oneOf(p.density, ["compact", "default", "spacious"], d.density),
    messageDisplay: oneOf(p.messageDisplay, ["cozy", "compact"], d.messageDisplay),
    chatFontSize: clamp(p.chatFontSize, 12, 20, d.chatFontSize),
    zoom: clamp(p.zoom, 80, 150, d.zoom),
    reduceMotion: oneOf(p.reduceMotion, ["system", "always", "never"], d.reduceMotion),
    saturation: clamp(p.saturation, 0, 100, d.saturation),
    roleColors: oneOf(p.roleColors, ["names", "beside", "off"], d.roleColors),
    // A language this build doesn't ship stays picked (a newer build may have it) and shows as English.
    language: typeof p.language === "string" && (p.language === "auto" || isTag(p.language)) ? p.language : d.language,
    clock: oneOf(p.clock, ["auto", "12h", "24h"], d.clock),
    sendWith: oneOf(p.sendWith, ["enter", "mod-enter"], d.sendWith),
    notifyFor: oneOf(p.notifyFor, ["mentions", "all"], d.notifyFor),
    volume: clamp(p.volume, 0, 100, d.volume),
    inputDevice: typeof p.inputDevice === "string" ? p.inputDevice : "",
    outputDevice: typeof p.outputDevice === "string" ? p.outputDevice : "",
    videoDevice: typeof p.videoDevice === "string" ? p.videoDevice : "",
    popoutFit: oneOf(p.popoutFit, ["cover", "contain"], d.popoutFit),
    inputVolume: clamp(p.inputVolume, 0, 200, d.inputVolume),
    outputVolume: clamp(p.outputVolume, 0, 200, d.outputVolume),
    inputMode: oneOf(p.inputMode, ["voice", "ptt"], d.inputMode),
    sensitivity: clamp(p.sensitivity, -100, 0, d.sensitivity),
    pttRelease: clamp(p.pttRelease, 0, 2000, d.pttRelease),
    userVolumes:
      p.userVolumes && typeof p.userVolumes === "object"
        ? Object.fromEntries(Object.entries(p.userVolumes).filter(([, v]) => typeof v === "number" && v >= 0 && v <= 200))
        : {},
    shareReports: p.shareReports !== false,
    othersEffects: p.othersEffects !== false,
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

/** Every theme someone can pick: the built-ins, then their own. */
export const allThemes = (p: Prefs = prefs): (Theme | CustomTheme)[] => [...BUILTIN_THEMES, ...p.customThemes];

export const themeById = (id: string, p: Prefs = prefs): Theme | CustomTheme =>
  allThemes(p).find((t) => t.id === id) ?? BUILTIN_THEMES[0]!;

/** The theme on screen now: the picked one, or the light or dark pick when following the system. */
export function activeTheme(p: Prefs = prefs): Theme | CustomTheme {
  if (!p.followSystem) return themeById(p.theme, p);
  return themeById(systemDark() ? p.darkTheme : p.lightTheme, p);
}

/** What's behind the app now: the theme's own backdrop, or the app's. */
export function activeBackdrop(p: Prefs = prefs): Backdrop {
  const theme = activeTheme(p);
  return ("backdrop" in theme && theme.backdrop) || p.backdrop;
}

export const lightThemes = (p: Prefs = prefs) => allThemes(p).filter((t) => !isDark(t));
export const darkThemes = (p: Prefs = prefs) => allThemes(p).filter((t) => isDark(t));

/** Adds or replaces a custom theme. */
export function saveCustomTheme(theme: CustomTheme) {
  setPrefs((p) => {
    const at = p.customThemes.findIndex((t) => t.id === theme.id);
    const next = { ...theme, updatedAt: Date.now() };
    const customThemes = at === -1 ? [...p.customThemes, next] : p.customThemes.map((t) => (t.id === theme.id ? next : t));
    return { customThemes };
  });
}

/** Removes a custom theme; anything that used it goes back to the defaults. */
export function deleteCustomTheme(id: string) {
  setPrefs((p) => ({
    customThemes: p.customThemes.filter((t) => t.id !== id),
    theme: p.theme === id ? DEFAULT_PREFS.theme : p.theme,
    lightTheme: p.lightTheme === id ? DEFAULT_PREFS.lightTheme : p.lightTheme,
    darkTheme: p.darkTheme === id ? DEFAULT_PREFS.darkTheme : p.darkTheme,
  }));
}

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
