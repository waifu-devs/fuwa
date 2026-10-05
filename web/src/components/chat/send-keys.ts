import type { KeyboardEvent } from "react";
import { isMac } from "@/lib/keybinds";
import type { SendWith } from "@/lib/prefs";

/** Whether a key press sends, by the Chat setting: Enter, or Ctrl+Enter (Cmd+Return on a Mac). */
export function sendsMessage(e: KeyboardEvent<HTMLTextAreaElement>, sendWith: SendWith) {
  if (e.key !== "Enter" || e.nativeEvent.isComposing) return false;
  const mod = isMac ? e.metaKey : e.ctrlKey;
  return sendWith === "enter" ? !e.shiftKey && !mod : mod;
}
