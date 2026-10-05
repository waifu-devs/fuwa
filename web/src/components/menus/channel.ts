import {
  CheckCheckIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CopyPlusIcon,
  KeyRoundIcon,
  LinkIcon,
  PlusIcon,
  SettingsIcon,
  Trash2Icon,
  UserPlusIcon,
} from "lucide-react";
import { ChannelType, OverwriteTarget, Permission, type Channel } from "@/gen/fuwa/v1/types_pb";
import { createChannel, deleteChannel, markChannelsRead, run } from "@/fuwa/actions";
import { accessNow, getInstance } from "@/fuwa/hooks";
import { copyIdItem, notificationEntries, placeLink } from "@/components/menus/common";
import { attempt, confirmFirst } from "@/components/menus/dialogs";
import { i18n } from "@/i18n/i18n";
import { items, withExtensions, type MenuContexts, type MenuSection } from "@/lib/context-menu";
import { above, has, hasIn, outranks, standing, type Access } from "@/lib/permissions";
import { reportError } from "@/lib/reports";
import { copy, toast } from "@/lib/ui";

/** What the channel list opens for a channel or category; the menu only asks for it. */
export type ChannelMenuActions = {
  /** Invite people straight into the channel. */
  invite?: () => void;
  /** Opens the channel's settings; "permissions" scrolls to who can see and use it. */
  edit: (channelId: string, focus?: "permissions") => void;
  /** A new channel in a category. */
  create?: (parentId: string) => void;
  /** Folds a category away or opens it. */
  toggle?: () => void;
  collapsed?: boolean;
};

const texty = (c: Channel) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT || c.type === ChannelType.SECURE;

/**
 * A copy of a channel: its name, kind, category, topic, slow mode and who can
 * see it, all made in one step, so the copy is never seen with other
 * permissions than the original's.
 */
async function duplicate(instanceKey: string, serverId: string, channel: Channel) {
  try {
    await run(
      createChannel(instanceKey, serverId, channel.name, channel.type, channel.parentId, {
        topic: channel.topic,
        slowmodeSeconds: channel.slowmodeSeconds,
        permissionOverwrites: channel.permissionOverwrites,
      }),
    );
  } catch (err) {
    reportError("context_menu.duplicate_channel", "channel");
    throw err;
  }
  toast(i18n().t("workspace.menu.channel.duplicated", { name: channel.type === ChannelType.VOICE ? channel.name : `#${channel.name}` }));
}

/**
 * Whether every overwrite on a channel is one you could set: for @everyone,
 * yourself, or roles and members below you, as the server checks.
 */
function outranksOverwrites(instanceKey: string, serverId: string, channel: Channel, access: Access) {
  const inst = getInstance(instanceKey);
  const roles = inst?.roles[serverId] ?? [];
  const ownerId = inst?.servers.find((s) => s.id === serverId)?.ownerId ?? "";
  const meId = inst?.me?.id;
  const members = new Map((inst?.members[serverId] ?? []).map((m) => [m.user?.id ?? "", m]));
  const positions = new Map(roles.map((r) => [r.id, r.position]));
  return channel.permissionOverwrites.every((o) => {
    if (o.targetId === serverId || o.targetId === meId) return true;
    if (o.target === OverwriteTarget.MEMBER) {
      const member = members.get(o.targetId);
      return !member || outranks(access, standing(ownerId, roles, member));
    }
    const position = positions.get(o.targetId);
    return position === undefined || above(access, position);
  });
}

/**
 * A channel's menu in the channel list. Sections: primary (mark read, invite,
 * copy link), notifications, manage (edit, permissions, duplicate),
 * developer, danger (delete).
 */
export function channelMenu(ctx: MenuContexts["channel"], actions: ChannelMenuActions): MenuSection[] {
  const { instanceKey, serverId, channel } = ctx;
  const inst = getInstance(instanceKey);
  const access = accessNow(instanceKey, serverId);
  const unread = inst?.unread[channel.id] ?? 0;
  const manages = hasIn(access, channel.id, Permission.MANAGE_CHANNELS);
  const roles = hasIn(access, channel.id, Permission.MANAGE_ROLES);
  // Making the copy needs Manage Channels where it goes; copying who may see it needs Manage Roles there.
  const createsHere = channel.parentId ? hasIn(access, channel.parentId, Permission.MANAGE_CHANNELS) : has(access, Permission.MANAGE_CHANNELS);
  // Copying who may see it needs Manage Roles where the copy goes, and to outrank everyone it names, as the server checks.
  const rolesThere = channel.parentId ? hasIn(access, channel.parentId, Permission.MANAGE_ROLES) : has(access, Permission.MANAGE_ROLES);
  const copies =
    createsHere &&
    channel.type !== ChannelType.SECURE &&
    !channel.shared &&
    (!channel.permissionOverwrites.length || (rolesThere && outranksOverwrites(instanceKey, serverId, channel, access)));
  const name = channel.type === ChannelType.VOICE ? channel.name : `#${channel.name}`;
  const { t } = i18n();
  return withExtensions("channel", ctx, [
    {
      id: "primary",
      items: items(
        texty(channel) && { id: "mark-read", label: t("workspace.menu.markRead"), icon: CheckCheckIcon, disabled: !unread, onSelect: () => markChannelsRead(instanceKey, [channel.id]) },
        actions.invite && { id: "invite", label: t("workspace.menu.invite"), icon: UserPlusIcon, onSelect: actions.invite },
        { id: "copy-link", label: t("workspace.menu.copyLink"), icon: LinkIcon, onSelect: () => copy(t, placeLink(instanceKey, serverId, channel.id), t("common.copy.channelLink")) },
      ),
    },
    { id: "notifications", items: texty(channel) ? notificationEntries(instanceKey, serverId, channel.id, "channel") : [] },
    {
      id: "manage",
      items: items(
        (manages || roles) && { id: "edit", label: t("workspace.menu.channel.edit"), icon: SettingsIcon, onSelect: () => actions.edit(channel.id) },
        roles && { id: "permissions", label: t("workspace.menu.permissions"), icon: KeyRoundIcon, onSelect: () => actions.edit(channel.id, "permissions") },
        copies && { id: "duplicate", label: t("workspace.menu.channel.duplicate"), icon: CopyPlusIcon, onSelect: () => attempt(duplicate(instanceKey, serverId, channel)) },
      ),
    },
    { id: "developer", items: items(copyIdItem(channel.id, "channel")) },
    {
      id: "danger",
      items: items(
        manages && {
          id: "delete",
          label: t("workspace.menu.channel.delete"),
          icon: Trash2Icon,
          danger: true,
          onSelect: () =>
            confirmFirst({
              title: t("workspace.menu.channel.deleteTitle", { name }),
              body: t("workspace.menu.channel.deleteBody"),
              action: t("workspace.menu.channel.delete"),
              run: () => run(deleteChannel(instanceKey, serverId, channel.id)).then(() => toast(t("workspace.menu.channel.deleted", { name }))),
            }),
        },
      ),
    },
  ]);
}

/** A category's menu: mark its channels read, fold it, add a channel, edit or delete it. */
export function categoryMenu(ctx: MenuContexts["category"], actions: ChannelMenuActions): MenuSection[] {
  const { instanceKey, serverId, category } = ctx;
  const inst = getInstance(instanceKey);
  const access = accessNow(instanceKey, serverId);
  const children = (inst?.channels[serverId] ?? []).filter((c) => c.parentId === category.id).map((c) => c.id);
  const unread = children.some((id) => (inst?.unread[id] ?? 0) > 0);
  const manages = hasIn(access, category.id, Permission.MANAGE_CHANNELS);
  const roles = hasIn(access, category.id, Permission.MANAGE_ROLES);
  const { t } = i18n();
  return withExtensions("category", ctx, [
    {
      id: "primary",
      items: items(
        { id: "mark-read", label: t("workspace.menu.markRead"), icon: CheckCheckIcon, disabled: !unread, onSelect: () => markChannelsRead(instanceKey, children) },
        actions.toggle && {
          id: "toggle",
          label: actions.collapsed ? t("workspace.menu.category.expand") : t("workspace.menu.category.collapse"),
          icon: actions.collapsed ? ChevronRightIcon : ChevronDownIcon,
          onSelect: actions.toggle,
        },
        manages && actions.create && { id: "create", label: t("workspace.menu.category.create"), icon: PlusIcon, onSelect: () => actions.create?.(category.id) },
      ),
    },
    {
      id: "manage",
      items: items(
        (manages || roles) && { id: "edit", label: t("workspace.menu.category.edit"), icon: SettingsIcon, onSelect: () => actions.edit(category.id) },
        roles && { id: "permissions", label: t("workspace.menu.permissions"), icon: KeyRoundIcon, onSelect: () => actions.edit(category.id, "permissions") },
      ),
    },
    { id: "developer", items: items(copyIdItem(category.id, "category")) },
    {
      id: "danger",
      items: items(
        manages && {
          id: "delete",
          label: t("workspace.menu.category.delete"),
          icon: Trash2Icon,
          danger: true,
          onSelect: () =>
            confirmFirst({
              title: t("workspace.menu.category.deleteTitle", { name: category.name }),
              body: t("workspace.menu.category.deleteBody"),
              action: t("workspace.menu.category.delete"),
              run: () => run(deleteChannel(instanceKey, serverId, category.id)).then(() => toast(t("workspace.menu.category.deleted", { name: category.name }))),
            }),
        },
      ),
    },
  ]);
}
