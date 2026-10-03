import { useMemo } from "react";
import { Permission } from "@/gen/fuwa/v1/types_pb";
import { useAccess } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { bit, has, type Access } from "@/lib/permissions";

/*
 * Which server settings sections you can open. Apart from the dialog, so the
 * server menu can list them without loading the settings themselves.
 */

/** Every section of server settings, and whether your permissions open it. Instance admins also get the server's caps, and can delete it. */
const SECTION_RULES: Record<string, (a: Access, instanceAdmin: boolean) => boolean> = {
  overview: (a) => has(a, Permission.MANAGE_SERVER),
  access: (a) => has(a, Permission.MANAGE_SERVER),
  // Who signs members in decides who gets in, so only the owner picks it.
  sso: (a) => a.owner,
  "join-form": (a) => has(a, Permission.MANAGE_SERVER),
  welcome: (a) => has(a, Permission.MANAGE_SERVER),
  invites: (a) =>
    has(a, Permission.MANAGE_SERVER) || has(a, Permission.CREATE_INVITE) || [...a.channels.values()].some((bits) => bits & bit(Permission.CREATE_INVITE)),
  roles: (a) => has(a, Permission.MANAGE_ROLES),
  channels: (a) => [...a.channels.values()].some((bits) => bits & (bit(Permission.MANAGE_CHANNELS) | bit(Permission.MANAGE_ROLES))),
  emoji: (a) => has(a, Permission.MANAGE_EMOJI),
  integrations: (a) => has(a, Permission.MANAGE_WEBHOOKS) || has(a, Permission.MANAGE_SERVER),
  usage: (a, admin) => admin || has(a, Permission.MANAGE_SERVER),
  limits: (_, admin) => admin,
  applications: (a) => has(a, Permission.KICK_MEMBERS),
  members: (a) =>
    [Permission.MANAGE_ROLES, Permission.MANAGE_NICKNAMES, Permission.KICK_MEMBERS, Permission.BAN_MEMBERS, Permission.TIME_OUT_MEMBERS].some((p) =>
      has(a, p),
    ),
  bans: (a) => has(a, Permission.BAN_MEMBERS),
  automod: (a) => has(a, Permission.MANAGE_SERVER),
  "audit-log": (a) => has(a, Permission.VIEW_AUDIT_LOG),
  ownership: (a) => a.owner,
  danger: (a, admin) => a.owner || admin,
};

/** The server settings sections you can open, in menu order. */
export function useServerSettingsTabs(instanceKey: string, serverId: string): string[] {
  const access = useAccess(instanceKey, serverId);
  const admin = useFuwa((s) => !!s.instances[instanceKey]?.admin);
  return useMemo(() => Object.keys(SECTION_RULES).filter((id) => SECTION_RULES[id]!(access, admin)), [access, admin]);
}
