import type { Dispatch, RefObject, SetStateAction } from "react";

/** Puts `piece` at a text box's caret, with a space before it when it would touch a word. */
export function insertAtCaret(box: RefObject<HTMLTextAreaElement | null>, setText: Dispatch<SetStateAction<string>>, piece: string) {
  const el = box.current;
  const value = el?.value ?? "";
  const start = el?.selectionStart ?? value.length;
  const end = el?.selectionEnd ?? value.length;
  const gap = start > 0 && !/\s$/.test(value.slice(0, start)) ? " " : "";
  setText(value.slice(0, start) + gap + piece + value.slice(end));
  const at = start + gap.length + piece.length;
  requestAnimationFrame(() => {
    el?.focus();
    el?.setSelectionRange(at, at);
  });
}
