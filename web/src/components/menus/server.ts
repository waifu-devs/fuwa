import { CheckCheckIcon, DoorOpenIcon, IdCardIcon, SettingsIcon, UserPlusIcon } from "lucide-react";
import { i18n } from "@/i18n/i18n";
import { Permission } from "@/gen/fuwa/v1/types_pb";
import { leaveServer, markServerRead, run } from "@/fuwa/actions";
import { accessNow, getInstance } from "@/fuwa/hooks";
import { openableChannels } from "@/components/channel-groups";
import { settingsTabsFor } from "@/components/dialogs/serverSettingsTabs";
import { copyIdItem, goTo, notificationEntries } from "@/components/menus/common";
import { confirmFirst, openMenuDialog } from "@/components/menus/dialogs";
import { items, withExtensions, type MenuContexts, type MenuSection } from "@/lib/context-menu";
import { effectiveNotifications } from "@/lib/notifications";
import { has, hasIn } from "@/lib/permissions";
import { openSettings } from "@/lib/ui";

/**
 * A server's menu in the rail, the server header menu's main items without
 * opening the server: mark read, invite, notifications, settings, your
 * server profile, leave.
 */
export function serverMenu(ctx: MenuContexts["server"]): MenuSection[] {
  const { instanceKey, server } = ctx;
  const inst = getInstance(instanceKey);
  const access = accessNow(instanceKey, server.id);
  const channels = inst?.channels[server.id] ?? [];
  const now = Date.now();
  const unread = !!inst && channels.some((c) => inst.unread[c.id] && !effectiveNotifications(inst, server.id, c.id, now).muted);
  // Invites go to the whole server where you may; else into the first channel you may invite to, as the header's menu would from there.
  const inviteTo = has(access, Permission.CREATE_INVITE)
    ? ""
    : (openableChannels(channels).find((c) => hasIn(access, c.id, Permission.CREATE_INVITE))?.id ?? null);
  const tabs = settingsTabsFor(access, !!inst?.admin);
  const { t } = i18n();
  return withExtensions("server", ctx, [
    {
      id: "primary",
      items: items(
        { id: "mark-read", label: t("workspace.menu.markRead"), icon: CheckCheckIcon, disabled: !unread, onSelect: () => markServerRead(instanceKey, server.id) },
        inviteTo !== null && {
          id: "invite",
          label: t("workspace.menu.invite"),
          icon: UserPlusIcon,
          onSelect: () => openMenuDialog({ kind: "invite", instanceKey, serverId: server.id, channelId: inviteTo }),
        },
      ),
    },
    { id: "notifications", items: notificationEntries(instanceKey, server.id, "", "server") },
    {
      id: "manage",
      items: items(
        tabs.length > 0 && {
          id: "settings",
          label: t("workspace.menu.server.settings"),
          icon: SettingsIcon,
          onSelect: () => openMenuDialog({ kind: "server-settings", instanceKey, server, tab: tabs[0]! }),
        },
        { id: "server-profile", label: t("workspace.menu.server.editProfile"), icon: IdCardIcon, onSelect: () => openSettings("server-profiles", server.id) },
      ),
    },
    { id: "developer", items: items(copyIdItem(server.id, "server")) },
    {
      id: "danger",
      items: items(
        !access.owner && {
          id: "leave",
          label: t("workspace.menu.server.leave"),
          icon: DoorOpenIcon,
          danger: true,
          onSelect: () =>
            confirmFirst({
              title: t("workspace.menu.server.leaveTitle", { name: server.name }),
              body: t("workspace.menu.server.leaveBody"),
              action: t("workspace.menu.server.leave"),
              run: async () => {
                await run(leaveServer(instanceKey, server.id));
                // Off the server's pages, if you were on one.
                if (location.pathname.includes(`/${server.id}`)) goTo({ to: "/$instance", params: { instance: instanceKey } });
              },
            }),
        },
      ),
    },
  ]);
}
