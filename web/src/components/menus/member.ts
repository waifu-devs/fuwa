import { AtSignIcon, DoorOpenIcon, GavelIcon, HourglassIcon, IdCardIcon, MessageCircleIcon, PencilIcon, ShieldIcon, TimerOffIcon, UserRoundIcon } from "lucide-react";
import { Permission, type Member, type User } from "@/gen/fuwa/v1/types_pb";
import { giveRole, run, takeRole } from "@/fuwa/actions";
import { openConversation } from "@/fuwa/dms";
import { accessNow, getInstance } from "@/fuwa/hooks";
import { useContextMenu, type MenuTrigger } from "@/components/ContextMenu";
import { copyIdItem, goTo } from "@/components/menus/common";
import { attempt, openMenuDialog } from "@/components/menus/MenuDialogs";
import { moderationFor } from "@/components/ModerateDialog";
import { items, withExtensions, type MenuContexts, type MenuSection } from "@/lib/context-menu";
import { isAgent, timedOutUntil } from "@/lib/format";
import { above, cssColor, has } from "@/lib/permissions";
import { hasCommand, openSettings, runCommand } from "@/lib/ui";

/** The composer on screen takes text to put at its caret by this command, for "Mention". */
export const COMPOSER_INSERT = "composer.insert";

/**
 * Someone's menu, wherever their name or avatar is: the member list, chat,
 * a join line. Sections: primary (profile, message, mention), social (for
 * friends, later), manage (nickname, roles), moderate (time out, kick, ban),
 * developer. Everything checks what you may do the way their buttons do.
 */
export function memberMenu(ctx: MenuContexts["member"], trigger: MenuTrigger): MenuSection[] {
  const { instanceKey, serverId, user } = ctx;
  const inst = getInstance(instanceKey);
  const meId = inst?.me?.id;
  const me = user.id === meId;
  // Their membership as it is now, so the menu follows roles given while it's open.
  const member = serverId ? (inst?.members[serverId]?.find((m) => m.user?.id === user.id) ?? ctx.member) : undefined;
  const dms = inst?.dms.status;
  const canMessage = !me && !isAgent(user) && (dms === "ready" || dms === "starting");
  const server = serverId ? inst?.servers.find((s) => s.id === serverId) : undefined;
  const access = server ? accessNow(instanceKey, serverId) : null;
  const roles = server ? (inst?.roles[serverId] ?? []) : [];
  const allowed = access && server && member ? moderationFor(access, server.ownerId, roles, meId, member) : null;
  const assignable = access && member && has(access, Permission.MANAGE_ROLES) ? roles.filter((r) => r.id !== serverId && above(access, r.position)) : [];
  const timedOut = !!member && !!timedOutUntil(member, Date.now());
  const moderate = (action: "timeout" | "kick" | "ban" | "nickname") => () => member && openMenuDialog({ kind: "moderate", instanceKey, serverId, member, action });
  // A button that opens their profile card: the one right-clicked, when it is one.
  const card = trigger.element.matches("button, [role=button]") ? trigger.element : null;

  return withExtensions("member", { ...ctx, member }, [
    {
      id: "primary",
      items: items(
        card && { id: "profile", label: "Profile", icon: UserRoundIcon, onSelect: () => card.click() },
        canMessage && {
          id: "message",
          label: "Message",
          icon: MessageCircleIcon,
          hint: "Encrypted",
          onSelect: () => attempt(run(openConversation(instanceKey, user.id)).then((conversation) => goTo({ to: "/$instance/dm/$conversation", params: { instance: instanceKey, conversation } }))),
        },
        !!serverId && hasCommand(COMPOSER_INSERT) && { id: "mention", label: "Mention", icon: AtSignIcon, onSelect: () => runCommand(COMPOSER_INSERT, `@${user.username} `) },
      ),
    },
    { id: "social", items: [] },
    {
      id: "manage",
      items: items(
        me && !!server && { id: "server-profile", label: "Edit server profile", icon: IdCardIcon, onSelect: () => openSettings("server-profiles", serverId) },
        !me && allowed?.nickname && { id: "nickname", label: "Change nickname", icon: PencilIcon, onSelect: moderate("nickname") },
        assignable.length > 0 &&
          !!member && {
            kind: "sub",
            id: "roles",
            label: "Roles",
            icon: ShieldIcon,
            hint: String(member.roleIds.filter((id) => id !== serverId).length || ""),
            items: assignable.map((role) => ({
              kind: "check" as const,
              id: role.id,
              label: role.name,
              color: role.color !== undefined ? cssColor(role.color) : undefined,
              checked: member.roleIds.includes(role.id),
              keepOpen: true,
              onSelect: () => attempt(run((member.roleIds.includes(role.id) ? takeRole : giveRole)(instanceKey, serverId, user.id, role.id))),
            })),
          },
      ),
    },
    {
      id: "moderate",
      items: items(
        allowed?.timeout && { id: "timeout", label: timedOut ? "End time out" : "Time out", icon: timedOut ? TimerOffIcon : HourglassIcon, danger: !timedOut, onSelect: moderate("timeout") },
        allowed?.kick && { id: "kick", label: "Kick", icon: DoorOpenIcon, danger: true, onSelect: moderate("kick") },
        allowed?.ban && { id: "ban", label: "Ban", icon: GavelIcon, danger: true, onSelect: moderate("ban") },
      ),
    },
    { id: "developer", items: items(copyIdItem(user.id, "user")) },
  ]);
}

/**
 * Right-click handlers for someone's name or avatar. Put them on the button
 * that opens their profile card, so "Profile" opens that card. Webhooks have
 * no profile and get none.
 */
export function useMemberMenu(instanceKey: string, serverId: string, user: User | undefined, member: Member | undefined) {
  return useContextMenu("member", (trigger) => (user ? memberMenu({ instanceKey, serverId, user, member }, trigger) : null));
}
