import type { I18n } from "@/i18n/react";
import { ACTIONS, actionName } from "@/lib/keybinds";

/** Where the Keybinds page's own rows sit for settings search. */
export const keybindSettings = (t: I18n["t"]) => [
  { id: "custom-keybinds", label: t("appsettings.keybinds.custom"), keywords: "add shortcut" },
  ...ACTIONS.map((a) => ({ id: `key-${a.id}`, label: actionName(t, a), keywords: "shortcut hotkey" })),
];
