import { CopyIcon, MessageSquareReplyIcon, PencilIcon, PinIcon, PinOffIcon, SmileIcon, SmilePlusIcon, Trash2Icon } from "lucide-react";
import type { MenuTrigger } from "@/components/ContextMenu";
import { targetSection, type MessageMenuActions } from "@/components/menus/message";
import { i18n } from "@/i18n/i18n";
import { items, withExtensions, type MenuContexts, type MenuSection } from "@/lib/context-menu";

/** What a direct message's (or secure channel's) own buttons can do; the moderation a channel has doesn't apply. */
export type DmMessageMenuActions = Pick<MessageMenuActions, "thread" | "edit" | "copyText" | "pin" | "react" | "reactions" | "delete">;

/**
 * A line's menu in a direct message or secure channel, with the same
 * sections as a channel message's: target, react, primary, manage, danger.
 * Lines have no ID of their own to copy (they're places in the
 * conversation), so there's no developer section.
 */
export function dmMessageMenu(ctx: MenuContexts["dm_message"], trigger: MenuTrigger, actions: DmMessageMenuActions): MenuSection[] {
  const { t } = i18n();
  return [
    targetSection(trigger),
    ...withExtensions("dm_message", ctx, [
      {
        id: "react",
        items: items(
          actions.react && { kind: "custom", id: "quick-reactions", render: actions.react.quick },
          actions.react?.add && { id: "add-reaction", label: t("chattools.reactions.add"), icon: SmilePlusIcon, onSelect: actions.react.add },
          actions.reactions && { id: "view-reactions", label: t("chattools.reactions.viewAll"), icon: SmileIcon, onSelect: actions.reactions.view },
        ),
      },
      {
        id: "primary",
        items: items(
          actions.thread && { id: "thread", label: actions.thread.open ? t("workspace.menu.message.openThread") : t("workspace.menu.message.replyInThread"), icon: MessageSquareReplyIcon, onSelect: actions.thread.go },
          actions.edit && { id: "edit", label: t("workspace.menu.message.edit"), icon: PencilIcon, onSelect: actions.edit },
          actions.copyText && { id: "copy-text", label: t("workspace.menu.message.copyText"), icon: CopyIcon, onSelect: actions.copyText },
        ),
      },
      {
        id: "manage",
        items: items(
          actions.pin && {
            id: "pin",
            label: t(actions.pin.pinned ? "chattools.pins.unpinMessage" : "chattools.pins.pin"),
            icon: actions.pin.pinned ? PinOffIcon : PinIcon,
            onSelect: actions.pin.toggle,
          },
        ),
      },
      { id: "danger", items: items(actions.delete && { id: "delete", label: t("workspace.menu.message.delete"), icon: Trash2Icon, danger: true, onSelect: actions.delete }) },
    ]),
  ].filter((s) => s.items.length > 0);
}
