import { ClipboardPasteIcon, CopyIcon, ScissorsIcon, SmileIcon, TextCursorInputIcon } from "lucide-react";
import { i18n } from "@/i18n/i18n";
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
  const { t } = i18n();
  const write = (text: string) => navigator.clipboard?.writeText(text).catch(() => toast(t("workspace.menu.composer.copyFailed")));
  return withExtensions("composer", ctx, [
    {
      id: "edit",
      items: items(
        !!selected && {
          id: "cut",
          label: t("workspace.menu.composer.cut"),
          icon: ScissorsIcon,
          hint: comboLabel("Mod+X"),
          onSelect: () => {
            void write(selected);
            replaceSelection("");
          },
        },
        !!selected && { id: "copy", label: t("workspace.menu.composer.copy"), icon: CopyIcon, hint: comboLabel("Mod+C"), onSelect: () => void write(selected) },
        {
          id: "paste",
          label: t("workspace.menu.composer.paste"),
          icon: ClipboardPasteIcon,
          hint: comboLabel("Mod+V"),
          onSelect: () => {
            const read = navigator.clipboard?.readText?.();
            if (!read) return toast(t("workspace.menu.composer.pasteWithKeys", { keys: comboLabel("Mod+V") }));
            read.then(replaceSelection, () => {
              // The browser said no (or asked and was told no): the keyboard still pastes.
              reportError("context_menu.paste_blocked", "composer");
              toast(t("workspace.menu.composer.pasteWithKeys", { keys: comboLabel("Mod+V") }));
            });
          },
        },
        !!box.value && {
          id: "select-all",
          label: t("workspace.menu.composer.selectAll"),
          icon: TextCursorInputIcon,
          hint: comboLabel("Mod+A"),
          onSelect: () => {
            box.focus();
            box.select();
          },
        },
      ),
    },
    { id: "insert", items: [{ id: "emoji", label: t("workspace.menu.composer.emoji"), icon: SmileIcon, onSelect: actions.openEmoji }] },
    { id: "help", items: [{ kind: "note", id: "spelling", label: t("workspace.menu.composer.spelling") }] },
  ]);
}
