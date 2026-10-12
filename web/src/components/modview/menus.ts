import { FlagIcon, ShieldEllipsisIcon } from "lucide-react";
import { Permission } from "@/gen/fuwa/v1/types_pb";
import { accessNow, getInstance } from "@/fuwa/hooks";
import { openCountNow, openModView } from "@/fuwa/moderation";
import { openMenuDialog } from "@/components/menus/dialogs";
import { i18n } from "@/i18n/i18n";
import { instanceHas } from "@/lib/compat";
import { extendMenu, items } from "@/lib/context-menu";
import { mayReport } from "@/lib/mod-view";
import { has } from "@/lib/permissions";

/*
 * Mod View's items in the app's right-click menus (docs/context-menus.md):
 * "Report message" on others' messages and "Report" on others, for
 * everyone; "Open in Mod View" on people and "Mod View" on servers, for
 * whoever may use it. Only on instances that have it.
 */

const here = (instanceKey: string) => instanceHas(getInstance(instanceKey)?.node?.versions, "mod-view");
const moderates = (instanceKey: string, serverId: string) => has(accessNow(instanceKey, serverId), Permission.MODERATE);

extendMenu("message", {
  section: "danger",
  at: "start",
  build: ({ instanceKey, serverId, channel, message }) => {
    const inst = getInstance(instanceKey);
    if (!here(instanceKey) || !mayReport(channel, message, inst?.me?.id)) return [];
    const { t } = i18n();
    return [
      {
        id: "report-message",
        label: t("chattools.report.messageItem"),
        icon: FlagIcon,
        danger: true,
        onSelect: () =>
          openMenuDialog({
            kind: "report",
            target: {
              kind: "message",
              instanceKey,
              serverId,
              channelId: message.channelId || channel.id,
              messageId: message.id,
              author: inst?.users[message.authorId] ?? message.shared?.user,
              content: message.content,
            },
          }),
      },
    ];
  },
});

extendMenu("member", {
  section: "moderate",
  at: "start",
  build: ({ instanceKey, serverId, user }) => {
    if (!serverId || !here(instanceKey) || !moderates(instanceKey, serverId)) return [];
    const server = getInstance(instanceKey)?.servers.find((s) => s.id === serverId);
    if (!server) return [];
    return [{ id: "mod-view", label: i18n().t("chattools.modview.openPerson"), icon: ShieldEllipsisIcon, onSelect: () => openModView(instanceKey, server, { userId: user.id }) }];
  },
});

extendMenu("member", {
  section: "danger",
  at: "start",
  build: ({ instanceKey, serverId, user }) => {
    if (!serverId || !here(instanceKey) || user.id === getInstance(instanceKey)?.me?.id) return [];
    return items({
      id: "report-member",
      label: i18n().t("chattools.report.memberItem"),
      icon: FlagIcon,
      danger: true,
      onSelect: () => openMenuDialog({ kind: "report", target: { kind: "member", instanceKey, serverId, user } }),
    });
  },
});

extendMenu("server", {
  section: "manage",
  at: "start",
  build: ({ instanceKey, server }) => {
    if (!here(instanceKey) || !moderates(instanceKey, server.id)) return [];
    const open = openCountNow(instanceKey, server.id);
    const { t, number } = i18n();
    return [{ id: "mod-view", label: t("chattools.modview.title"), icon: ShieldEllipsisIcon, hint: open ? number(open) : "", onSelect: () => openModView(instanceKey, server) }];
  },
});
