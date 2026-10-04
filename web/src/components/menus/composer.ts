import { ClipboardPasteIcon, CopyIcon, ScissorsIcon, SmileIcon, TextCursorInputIcon } from "lucide-react";
import { items, withExtensions, type MenuContexts, type MenuSection } from "@/lib/context-menu";
import { reportError } from "@/lib/reports";
import { comboLabel } from "@/lib/keybinds";
import { toast } from "@/lib/ui";

/** What the composer does for its menu: change its text, open its emoji picker. */
export type ComposerMenuActions = {
  box: HTMLTextAreaElement;
  /** Replaces the selection with `text` and puts the caret after it. */
  replaceSelection: (text: string) => void;
  openEmoji: () => void;
};

/**
 * The message box's menu: cut, copy, paste, select all, emoji. Typing
 * features (polls, attachments) add theirs to "insert". The browser's own
 * menu, with spelling suggestions, is a Shift + right click away.
 */
export function composerMenu(ctx: MenuContexts["composer"], actions: ComposerMenuActions): MenuSection[] {
  const { box, replaceSelection } = actions;
  const selected = box.value.slice(box.selectionStart, box.selectionEnd);
  const write = (text: string) => navigator.clipboard?.writeText(text).catch(() => toast("Couldn't copy"));
  return withExtensions("composer", ctx, [
    {
      id: "edit",
      items: items(
        !!selected && {
          id: "cut",
          label: "Cut",
          icon: ScissorsIcon,
          hint: comboLabel("Mod+X"),
          onSelect: () => {
            void write(selected);
            replaceSelection("");
          },
        },
        !!selected && { id: "copy", label: "Copy", icon: CopyIcon, hint: comboLabel("Mod+C"), onSelect: () => void write(selected) },
        {
          id: "paste",
          label: "Paste",
          icon: ClipboardPasteIcon,
          hint: comboLabel("Mod+V"),
          onSelect: () => {
            const read = navigator.clipboard?.readText?.();
            if (!read) return toast(`Paste with ${comboLabel("Mod+V")} in this browser`);
            read.then(replaceSelection, () => {
              // The browser said no (or asked and was told no): the keyboard still pastes.
              reportError("context_menu.paste_blocked", "composer");
              toast(`Paste with ${comboLabel("Mod+V")} in this browser`);
            });
          },
        },
        !!box.value && {
          id: "select-all",
          label: "Select all",
          icon: TextCursorInputIcon,
          hint: comboLabel("Mod+A"),
          onSelect: () => {
            box.focus();
            box.select();
          },
        },
      ),
    },
    { id: "insert", items: [{ id: "emoji", label: "Emoji", icon: SmileIcon, onSelect: actions.openEmoji }] },
    { id: "help", items: [{ kind: "note", id: "spelling", label: "Shift + right click for spelling fixes" }] },
  ]);
}
