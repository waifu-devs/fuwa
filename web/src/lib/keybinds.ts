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
  /** Its name in the catalog (locales/, appsettings.keybinds.action.*). */
  name: Key;
  group: KeyGroup;
  /** Null when the action has no shortcut until someone gives it one. */
  combo: string | null;
  /** Works while typing in a text box too. */
  whileTyping?: boolean;
};

export const ACTIONS: KeyAction[] = [
  { id: "quickSwitcher", name: "appsettings.keybinds.action.quickSwitcher", group: "Navigation", combo: "Mod+K", whileTyping: true },
  { id: "searchServer", name: "appsettings.keybinds.action.searchServer", group: "Navigation", combo: "Mod+F", whileTyping: true },
  { id: "previousServer", name: "appsettings.keybinds.action.previousServer", group: "Navigation", combo: "Mod+Alt+ArrowUp", whileTyping: true },
  { id: "nextServer", name: "appsettings.keybinds.action.nextServer", group: "Navigation", combo: "Mod+Alt+ArrowDown", whileTyping: true },
  { id: "previousChannel", name: "appsettings.keybinds.action.previousChannel", group: "Navigation", combo: "Alt+ArrowUp", whileTyping: true },
  { id: "nextChannel", name: "appsettings.keybinds.action.nextChannel", group: "Navigation", combo: "Alt+ArrowDown", whileTyping: true },
  { id: "previousUnread", name: "appsettings.keybinds.action.previousUnread", group: "Navigation", combo: "Alt+Shift+ArrowUp", whileTyping: true },
  { id: "nextUnread", name: "appsettings.keybinds.action.nextUnread", group: "Navigation", combo: "Alt+Shift+ArrowDown", whileTyping: true },
  { id: "markServerRead", name: "appsettings.keybinds.action.markServerRead", group: "Messages", combo: "Shift+Escape", whileTyping: true },
  { id: "focusComposer", name: "appsettings.keybinds.action.focusComposer", group: "Chat", combo: "Tab" },
  { id: "insertTimestamp", name: "appsettings.keybinds.action.insertTimestamp", group: "Chat", combo: "Alt+Shift+T", whileTyping: true },
  { id: "toggleMembers", name: "appsettings.keybinds.action.toggleMembers", group: "Chat", combo: "Mod+U", whileTyping: true },
  { id: "toggleMute", name: "appsettings.keybinds.action.toggleMute", group: "Voice", combo: "Mod+Shift+M", whileTyping: true },
  { id: "toggleDeafen", name: "appsettings.keybinds.action.toggleDeafen", group: "Voice", combo: "Mod+Shift+D", whileTyping: true },
  { id: "pushToTalk", name: "appsettings.keybinds.action.pushToTalk", group: "Voice", combo: null },
  { id: "toggleCamera", name: "appsettings.keybinds.action.toggleCamera", group: "Voice", combo: null, whileTyping: true },
  { id: "toggleScreen", name: "appsettings.keybinds.action.toggleScreen", group: "Voice", combo: null, whileTyping: true },
  { id: "toggleRecording", name: "appsettings.keybinds.action.toggleRecording", group: "Voice", combo: null, whileTyping: true },
  { id: "openSettings", name: "appsettings.keybinds.action.openSettings", group: "App", combo: "Mod+Comma", whileTyping: true },
  { id: "shortcuts", name: "appsettings.keybinds.action.shortcuts", group: "App", combo: "Mod+Slash", whileTyping: true },
  { id: "toggleStreamer", name: "appsettings.keybinds.action.toggleStreamer", group: "App", combo: null, whileTyping: true },
];

export const GROUPS: KeyGroup[] = ["Navigation", "Messages", "Chat", "Voice", "App"];

const GROUP_NAMES: Record<KeyGroup, Key> = {
  Navigation: "appsettings.keybinds.group.navigation",
  Messages: "appsettings.keybinds.group.messages",
  Chat: "appsettings.keybinds.group.chat",
  Voice: "appsettings.keybinds.group.voice",
  App: "appsettings.keybinds.group.app",
};

/** An action's name in the app's language. */
export const actionName = (t: I18n["t"], action: KeyAction) => t(action.name);

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
