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
import { ChannelType, Permission, type Channel } from "@/gen/fuwa/v1/types_pb";
import { createChannel, deleteChannel, markChannelsRead, run, setChannelPermissions, updateChannel } from "@/fuwa/actions";
import { accessNow, getInstance } from "@/fuwa/hooks";
import { copyIdItem, notificationEntries, placeLink } from "@/components/menus/common";
import { attempt, confirmFirst } from "@/components/menus/MenuDialogs";
import { items, withExtensions, type MenuContexts, type MenuSection } from "@/lib/context-menu";
import { has, hasIn } from "@/lib/permissions";
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
 * see it. A private channel's copy gets its permissions too, or it's taken
 * back, so a copy never opens up what the original kept closed.
 */
async function duplicate(instanceKey: string, serverId: string, channel: Channel) {
  const made = await run(createChannel(instanceKey, serverId, channel.name, channel.type, channel.parentId));
  try {
    if (channel.topic || channel.slowmodeSeconds)
      await run(updateChannel(instanceKey, serverId, made.id, { topic: channel.topic, slowmodeSeconds: channel.slowmodeSeconds }));
    if (channel.permissionOverwrites.length)
      await run(
        setChannelPermissions(
          instanceKey,
          serverId,
          made.id,
          channel.permissionOverwrites.map(({ targetId, target, allow, deny }) => ({ targetId, target, allow, deny })),
        ),
      );
  } catch (err) {
    reportError("context_menu.duplicate_channel", "channel");
    await run(deleteChannel(instanceKey, serverId, made.id)).catch(() => {});
    throw err;
  }
  toast(`Made a copy of ${channel.type === ChannelType.VOICE ? channel.name : `#${channel.name}`}`);
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
  const copies = createsHere && channel.type !== ChannelType.SECURE && !channel.shared && (!channel.permissionOverwrites.length || roles);
  const name = channel.type === ChannelType.VOICE ? channel.name : `#${channel.name}`;
  return withExtensions("channel", ctx, [
    {
      id: "primary",
      items: items(
        texty(channel) && { id: "mark-read", label: "Mark as read", icon: CheckCheckIcon, disabled: !unread, onSelect: () => markChannelsRead(instanceKey, [channel.id]) },
        actions.invite && { id: "invite", label: "Invite people", icon: UserPlusIcon, onSelect: actions.invite },
        { id: "copy-link", label: "Copy link", icon: LinkIcon, onSelect: () => copy(placeLink(instanceKey, serverId, channel.id), "channel link") },
      ),
    },
    { id: "notifications", items: texty(channel) ? notificationEntries(instanceKey, serverId, channel.id, "channel") : [] },
    {
      id: "manage",
      items: items(
        (manages || roles) && { id: "edit", label: "Edit channel", icon: SettingsIcon, onSelect: () => actions.edit(channel.id) },
        roles && { id: "permissions", label: "Permissions", icon: KeyRoundIcon, onSelect: () => actions.edit(channel.id, "permissions") },
        copies && { id: "duplicate", label: "Duplicate channel", icon: CopyPlusIcon, onSelect: () => attempt(duplicate(instanceKey, serverId, channel)) },
      ),
    },
    { id: "developer", items: items(copyIdItem(channel.id, "channel")) },
    {
      id: "danger",
      items: items(
        manages && {
          id: "delete",
          label: "Delete channel",
          icon: Trash2Icon,
          danger: true,
          onSelect: () =>
            confirmFirst({
              title: `Delete ${name}?`,
              body: "Every message in it goes too, for everyone. This can't be undone.",
              action: "Delete channel",
              run: () => run(deleteChannel(instanceKey, serverId, channel.id)).then(() => toast(`Deleted ${name}`)),
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
  return withExtensions("category", ctx, [
    {
      id: "primary",
      items: items(
        { id: "mark-read", label: "Mark as read", icon: CheckCheckIcon, disabled: !unread, onSelect: () => markChannelsRead(instanceKey, children) },
        actions.toggle && {
          id: "toggle",
          label: actions.collapsed ? "Expand category" : "Collapse category",
          icon: actions.collapsed ? ChevronRightIcon : ChevronDownIcon,
          onSelect: actions.toggle,
        },
        manages && actions.create && { id: "create", label: "Create channel", icon: PlusIcon, onSelect: () => actions.create?.(category.id) },
      ),
    },
    {
      id: "manage",
      items: items(
        (manages || roles) && { id: "edit", label: "Edit category", icon: SettingsIcon, onSelect: () => actions.edit(category.id) },
        roles && { id: "permissions", label: "Permissions", icon: KeyRoundIcon, onSelect: () => actions.edit(category.id, "permissions") },
      ),
    },
    { id: "developer", items: items(copyIdItem(category.id, "category")) },
    {
      id: "danger",
      items: items(
        manages && {
          id: "delete",
          label: "Delete category",
          icon: Trash2Icon,
          danger: true,
          onSelect: () =>
            confirmFirst({
              title: `Delete ${category.name}?`,
              body: "Its channels stay, outside any category.",
              action: "Delete category",
              run: () => run(deleteChannel(instanceKey, serverId, category.id)).then(() => toast(`Deleted ${category.name}`)),
            }),
        },
      ),
    },
  ]);
}
