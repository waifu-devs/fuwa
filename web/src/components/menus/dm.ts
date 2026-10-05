import { CheckCheckIcon } from "lucide-react";
import { i18n } from "@/i18n/i18n";
import { markDmRead } from "@/fuwa/dms";
import { getInstance } from "@/fuwa/hooks";
import { copyIdItem } from "@/components/menus/common";
import { items, withExtensions, type MenuContexts, type MenuSection } from "@/lib/context-menu";

/** A conversation's menu in the direct messages list: mark it read; with Developer Mode, the other person's ID. */
export function dmMenu(ctx: MenuContexts["dm"]): MenuSection[] {
  const { instanceKey, conversation, other } = ctx;
  const unread = getInstance(instanceKey)?.dms.unread[conversation.id] ?? 0;
  const { t } = i18n();
  return withExtensions("dm", ctx, [
    {
      id: "primary",
      items: [{ id: "mark-read", label: t("workspace.menu.markRead"), icon: CheckCheckIcon, disabled: !unread, onSelect: () => void markDmRead(instanceKey, conversation.id) }],
    },
    { id: "developer", items: items(other && copyIdItem(other.id, "user")) },
  ]);
}
