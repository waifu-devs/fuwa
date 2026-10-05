import type { I18n, Key } from "@/i18n/i18n";
import { getPrefs, type Prefs } from "@/lib/prefs";

/**
 * Keyboard shortcuts. Every action has an id, a default combo and a place on
 * the Keybinds page; the app's key handler, the shortcut overlay and custom
 * binds all read from this one list.
 *
 * Combos are stored the same on every system: "Mod" is Cmd on macOS and
 * Ctrl elsewhere, then Ctrl (macOS only), Alt, Shift and the key, joined
 * with "+", such as "Mod+K" or "Alt+Shift+ArrowUp". Keys are physical
 * positions (event.code), so Alt+K stays Alt+K on a Mac.
 */

export type KeyGroup = "Navigation" | "Messages" | "Chat" | "Voice" | "App";

export type KeyAction = {
  id: string;
  label: string;
  group: KeyGroup;
  /** Null when the action has no shortcut until someone gives it one. */
  combo: string | null;
  /** Works while typing in a text box too. */
  whileTyping?: boolean;
};

export const ACTIONS: KeyAction[] = [
  { id: "quickSwitcher", label: "Find a server or channel", group: "Navigation", combo: "Mod+K", whileTyping: true },
  { id: "searchServer", label: "Search this server's messages", group: "Navigation", combo: "Mod+F", whileTyping: true },
  { id: "previousServer", label: "Previous server", group: "Navigation", combo: "Mod+Alt+ArrowUp", whileTyping: true },
  { id: "nextServer", label: "Next server", group: "Navigation", combo: "Mod+Alt+ArrowDown", whileTyping: true },
  { id: "previousChannel", label: "Previous channel", group: "Navigation", combo: "Alt+ArrowUp", whileTyping: true },
  { id: "nextChannel", label: "Next channel", group: "Navigation", combo: "Alt+ArrowDown", whileTyping: true },
  { id: "previousUnread", label: "Previous unread channel", group: "Navigation", combo: "Alt+Shift+ArrowUp", whileTyping: true },
  { id: "nextUnread", label: "Next unread channel", group: "Navigation", combo: "Alt+Shift+ArrowDown", whileTyping: true },
  { id: "markServerRead", label: "Mark the server as read", group: "Messages", combo: "Shift+Escape", whileTyping: true },
  { id: "focusComposer", label: "Start typing a message", group: "Chat", combo: "Tab" },
  { id: "insertTimestamp", label: "Insert a timestamp", group: "Chat", combo: "Alt+Shift+T", whileTyping: true },
  { id: "toggleMembers", label: "Show or hide members", group: "Chat", combo: "Mod+U", whileTyping: true },
  { id: "toggleMute", label: "Mute or unmute yourself", group: "Voice", combo: "Mod+Shift+M", whileTyping: true },
  { id: "toggleDeafen", label: "Deafen or undeafen yourself", group: "Voice", combo: "Mod+Shift+D", whileTyping: true },
  { id: "pushToTalk", label: "Push to talk (hold)", group: "Voice", combo: null },
  { id: "toggleCamera", label: "Turn your camera on or off", group: "Voice", combo: null, whileTyping: true },
  { id: "toggleScreen", label: "Share your screen, or stop", group: "Voice", combo: null, whileTyping: true },
  { id: "toggleRecording", label: "Record the call, or stop and save it", group: "Voice", combo: null, whileTyping: true },
  { id: "openSettings", label: "Open settings", group: "App", combo: "Mod+Comma", whileTyping: true },
  { id: "shortcuts", label: "Show keyboard shortcuts", group: "App", combo: "Mod+Slash", whileTyping: true },
  { id: "toggleStreamer", label: "Turn streamer mode on or off", group: "App", combo: null, whileTyping: true },
];

export const GROUPS: KeyGroup[] = ["Navigation", "Messages", "Chat", "Voice", "App"];

/** Each action's name in the catalog (locales/, appsettings.keybinds.action.*). */
const ACTION_NAMES: Record<string, Key> = {
  quickSwitcher: "appsettings.keybinds.action.quickSwitcher",
  searchServer: "appsettings.keybinds.action.searchServer",
  previousServer: "appsettings.keybinds.action.previousServer",
  nextServer: "appsettings.keybinds.action.nextServer",
  previousChannel: "appsettings.keybinds.action.previousChannel",
  nextChannel: "appsettings.keybinds.action.nextChannel",
  previousUnread: "appsettings.keybinds.action.previousUnread",
  nextUnread: "appsettings.keybinds.action.nextUnread",
  markServerRead: "appsettings.keybinds.action.markServerRead",
  focusComposer: "appsettings.keybinds.action.focusComposer",
  insertTimestamp: "appsettings.keybinds.action.insertTimestamp",
  toggleMembers: "appsettings.keybinds.action.toggleMembers",
  toggleMute: "appsettings.keybinds.action.toggleMute",
  toggleDeafen: "appsettings.keybinds.action.toggleDeafen",
  pushToTalk: "appsettings.keybinds.action.pushToTalk",
  toggleCamera: "appsettings.keybinds.action.toggleCamera",
  toggleScreen: "appsettings.keybinds.action.toggleScreen",
  toggleRecording: "appsettings.keybinds.action.toggleRecording",
  openSettings: "appsettings.keybinds.action.openSettings",
  shortcuts: "appsettings.keybinds.action.shortcuts",
  toggleStreamer: "appsettings.keybinds.action.toggleStreamer",
};

const GROUP_NAMES: Record<KeyGroup, Key> = {
  Navigation: "appsettings.keybinds.group.navigation",
  Messages: "appsettings.keybinds.group.messages",
  Chat: "appsettings.keybinds.group.chat",
  Voice: "appsettings.keybinds.group.voice",
  App: "appsettings.keybinds.group.app",
};

/** An action's name in the app's language (its English label if the catalog doesn't name it). */
export const actionName = (t: I18n["t"], action: KeyAction) => {
  const key = ACTION_NAMES[action.id];
  return key ? t(key) : action.label;
};

/** A group's heading in the app's language. */
export const groupName = (t: I18n["t"], group: KeyGroup) => t(GROUP_NAMES[group]);

export const actionById = (id: string) => ACTIONS.find((a) => a.id === id);

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

const MODIFIERS = ["Mod", "Ctrl", "Meta", "Alt", "Shift"] as const;

/** Codes whose key reads better as its character. */
const CODE_KEYS: Record<string, string> = {
  Comma: "Comma",
  Period: "Period",
  Slash: "Slash",
  Backslash: "Backslash",
  Semicolon: "Semicolon",
  Quote: "Quote",
  BracketLeft: "BracketLeft",
  BracketRight: "BracketRight",
  Minus: "Minus",
  Equal: "Equal",
  Backquote: "Backquote",
  Space: "Space",
};

/** The key part of a combo from a key event, or null for a bare modifier. */
function keyOf(e: KeyboardEvent): string | null {
  const code = e.code;
  if (/^(Shift|Control|Alt|Meta|OS)(Left|Right)?$/.test(code) || ["Shift", "Control", "Alt", "Meta"].includes(e.key)) return null;
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit\d$/.test(code)) return code.slice(5);
  if (/^Numpad\d$/.test(code)) return code.slice(6);
  if (CODE_KEYS[code]) return CODE_KEYS[code];
  if (/^(Arrow(Up|Down|Left|Right)|Escape|Tab|Enter|Backspace|Delete|Home|End|PageUp|PageDown|Insert|F\d{1,2})$/.test(code)) return code;
  return e.key.length === 1 ? e.key.toUpperCase() : e.key || null;
}

/** The combo a key event makes, or null while only modifiers are down. */
export function comboOf(e: KeyboardEvent): string | null {
  const key = keyOf(e);
  if (!key) return null;
  return [...modifiersOf(e), key].join("+");
}

/** The modifiers held in a key event, in combo order. */
export function modifiersOf(e: KeyboardEvent | { ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean }): string[] {
  const out: string[] = [];
  if (isMac ? e.metaKey : e.ctrlKey) out.push("Mod");
  if (isMac && e.ctrlKey) out.push("Ctrl");
  if (!isMac && e.metaKey) out.push("Meta");
  if (e.altKey) out.push("Alt");
  if (e.shiftKey) out.push("Shift");
  return out;
}

/** Puts a combo's parts in the one order, so equal combos compare equal. */
export function normalize(combo: string): string {
  const parts = combo.split("+");
  const key = parts.pop() ?? "";
  const mods = MODIFIERS.filter((m) => parts.includes(m));
  return [...mods, key].join("+");
}

const MAC_LABELS: Record<string, string> = { Mod: "⌘", Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Meta: "⌘" };
const PC_LABELS: Record<string, string> = { Mod: "Ctrl", Ctrl: "Ctrl", Alt: "Alt", Shift: "Shift", Meta: "Win" };
const KEY_LABELS: Record<string, string> = {
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Escape: "Esc",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
  Space: "Space",
  Enter: isMac ? "Return" : "Enter",
  Backspace: isMac ? "Delete" : "Backspace",
};

/** A combo as keycaps to show: ["Ctrl", "K"], or ["⌘", "K"] on a Mac. */
export function keycaps(combo: string): string[] {
  return combo.split("+").map((part) => (isMac ? MAC_LABELS : PC_LABELS)[part] ?? KEY_LABELS[part] ?? part);
}

/** A combo in words, for tooltips and messages. */
export const comboLabel = (combo: string) => keycaps(combo).join(isMac ? "" : "+");

/**
 * Combos a browser keeps for itself (new tab, close tab, quit...), so a page
 * never sees them. The desktop app can use them.
 */
const BROWSER_KEEPS = new Set([
  "Mod+W",
  "Mod+T",
  "Mod+N",
  "Mod+Q",
  "Mod+R",
  "Mod+L",
  "Mod+Shift+W",
  "Mod+Shift+T",
  "Mod+Shift+N",
  "Ctrl+Tab",
  "Ctrl+Shift+Tab",
  "Mod+Tab",
  "Mod+Shift+Tab",
]);

export const browserKeeps = (combo: string) => BROWSER_KEEPS.has(normalize(combo));

/** An action's shortcut now: its default, or what someone changed it to (null when unbound). */
export function bindingOf(action: KeyAction, p: Prefs = getPrefs()): string | null {
  return action.id in p.keybinds ? (p.keybinds[action.id] ?? null) : action.combo;
}

/** Every combo in use and the action it runs, defaults and custom binds together. */
export function bindings(p: Prefs = getPrefs()): Map<string, string> {
  const out = new Map<string, string>();
  for (const action of ACTIONS) {
    const combo = bindingOf(action, p);
    if (combo) out.set(normalize(combo), action.id);
  }
  for (const custom of p.customKeybinds) if (!out.has(normalize(custom.combo))) out.set(normalize(custom.combo), custom.action);
  return out;
}

/**
 * Why a combo can't be used for an action, or null if it can. `except` is
 * the binding being changed, so recording the same combo again is fine.
 * `t` is the app's language (i18n/).
 */
export function problemWith(combo: string, p: Prefs, except: { action?: string; custom?: string }, t: I18n["t"]): string | null {
  const c = normalize(combo);
  const keys = comboLabel(c);
  if (browserKeeps(c)) return t("appsettings.keybinds.browserKeeps", { keys });
  for (const action of ACTIONS) {
    if (action.id === except.action) continue;
    const bound = bindingOf(action, p);
    if (bound && normalize(bound) === c) return t("appsettings.keybinds.taken", { keys, action: actionName(t, action) });
  }
  for (const custom of p.customKeybinds) {
    if (custom.id === except.custom) continue;
    if (normalize(custom.combo) === c) {
      const action = actionById(custom.action);
      return t("appsettings.keybinds.taken", { keys, action: action ? actionName(t, action) : custom.action });
    }
  }
  return null;
}

/** Keys that only work inside the message box, listed on the overlay but not rebindable. `t` is the app's language. */
export function composerKeys(t: I18n["t"], sendWith: Prefs["sendWith"]): { id: string; label: string; combo: string }[] {
  return [
    { id: "send", label: t("chattools.shortcuts.composer.send"), combo: sendWith === "enter" ? "Enter" : "Mod+Enter" },
    { id: "newLine", label: t("chattools.shortcuts.composer.newLine"), combo: sendWith === "enter" ? "Shift+Enter" : "Enter" },
    { id: "editLast", label: t("chattools.shortcuts.composer.editLast"), combo: "ArrowUp" },
    { id: "stopEditing", label: t("chattools.shortcuts.composer.stopEditing"), combo: "Escape" },
  ];
}
