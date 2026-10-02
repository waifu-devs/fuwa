import {
  ArrowDownUpIcon,
  BotIcon,
  FrownIcon,
  PartyPopperIcon,
  ShieldAlertIcon,
  ShieldCheckIcon,
  SmileIcon,
  SmilePlusIcon,
  ArrowRightIcon,
  ChevronDownIcon,
  ClipboardListIcon,
  CrownIcon,
  DoorOpenIcon,
  FolderPlusIcon,
  GavelIcon,
  HashIcon,
  HourglassIcon,
  Link2OffIcon,
  LinkIcon,
  LoaderCircleIcon,
  LockIcon,
  MessageSquareXIcon,
  ScrollTextIcon,
  SettingsIcon,
  ShieldIcon,
  ShieldPlusIcon,
  ShieldXIcon,
  Trash2Icon,
  UndoIcon,
  UserCheckIcon,
  UserCogIcon,
  UserXIcon,
  type LucideIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { AuditAction, type AuditChange, type AuditEntry } from "@/gen/fuwa/v1/server_pb";
import { ChannelType, NotificationLevel, type Channel, type Permission, type Role, type User } from "@/gen/fuwa/v1/types_pb";
import { listAuditLog, run, type AuditFilter } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance, useRoles } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { displayName, formatDuration, formatStamp, toDate } from "@/lib/format";
import { cssColor, permissionLabel } from "@/lib/permissions";
import { cn } from "@/lib/utils";

type Kind = { label: string; icon: LucideIcon; tint: string };

const KINDS: Record<AuditAction, Kind> = {
  [AuditAction.UNSPECIFIED]: { label: "Anything", icon: ScrollTextIcon, tint: "bg-muted text-muted-foreground" },
  [AuditAction.SERVER_UPDATE]: { label: "Server settings", icon: SettingsIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.CHANNEL_CREATE]: { label: "New channels", icon: FolderPlusIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.CHANNEL_UPDATE]: { label: "Channel changes", icon: HashIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.CHANNEL_DELETE]: { label: "Deleted channels", icon: Trash2Icon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.CHANNELS_REORDER]: { label: "Channel order", icon: ArrowDownUpIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.CHANNEL_PERMISSIONS_UPDATE]: { label: "Channel permissions", icon: LockIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.ROLE_CREATE]: { label: "New roles", icon: ShieldPlusIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.ROLE_UPDATE]: { label: "Role changes", icon: ShieldIcon, tint: "bg-violet-500/15 text-violet-500" },
  [AuditAction.ROLE_DELETE]: { label: "Deleted roles", icon: ShieldXIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.ROLES_REORDER]: { label: "Role order", icon: ArrowDownUpIcon, tint: "bg-violet-500/15 text-violet-500" },
  [AuditAction.MEMBER_ROLES_UPDATE]: { label: "Roles given and taken", icon: UserCogIcon, tint: "bg-violet-500/15 text-violet-500" },
  [AuditAction.MEMBER_UPDATE]: { label: "Nicknames", icon: UserCogIcon, tint: "bg-violet-500/15 text-violet-500" },
  [AuditAction.MEMBER_TIME_OUT]: { label: "Time-outs", icon: HourglassIcon, tint: "bg-amber-500/15 text-amber-500" },
  [AuditAction.MEMBER_KICK]: { label: "Kicks", icon: DoorOpenIcon, tint: "bg-orange-500/15 text-orange-500" },
  [AuditAction.MEMBER_BAN]: { label: "Bans", icon: GavelIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.MEMBER_UNBAN]: { label: "Unbans", icon: UndoIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.MESSAGE_DELETE]: { label: "Deleted messages", icon: MessageSquareXIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.OWNERSHIP_TRANSFER]: { label: "Ownership", icon: CrownIcon, tint: "bg-amber-500/15 text-amber-500" },
  [AuditAction.INVITE_CREATE]: { label: "New invites", icon: LinkIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.INVITE_DELETE]: { label: "Revoked invites", icon: Link2OffIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.APPLICATION_APPROVE]: { label: "Applications let in", icon: UserCheckIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.APPLICATION_REJECT]: { label: "Applications turned down", icon: UserXIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.JOIN_FORM_UPDATE]: { label: "Rules and questions", icon: ClipboardListIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.WELCOME_SCREEN_UPDATE]: { label: "Welcome screen", icon: PartyPopperIcon, tint: "bg-pink-500/15 text-pink-500" },
  [AuditAction.AUTO_MOD_RULE_CREATE]: { label: "New AutoMod rules", icon: ShieldCheckIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.AUTO_MOD_RULE_UPDATE]: { label: "AutoMod changes", icon: ShieldAlertIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.AUTO_MOD_RULE_DELETE]: { label: "Deleted AutoMod rules", icon: ShieldXIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.AUTO_MOD_TIME_OUT]: { label: "AutoMod time-outs", icon: BotIcon, tint: "bg-amber-500/15 text-amber-500" },
  [AuditAction.EMOJI_CREATE]: { label: "New emoji", icon: SmilePlusIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.EMOJI_UPDATE]: { label: "Renamed emoji", icon: SmileIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.EMOJI_DELETE]: { label: "Deleted emoji", icon: FrownIcon, tint: "bg-destructive/15 text-destructive" },
};

const FIELD: Record<string, string> = {
  name: "Name",
  description: "Description",
  icon_url: "Icon",
  discoverable: "Shown in Browse",
  default_notifications: "Default notifications",
  system_channel_id: "Join messages",
  topic: "Topic",
  parent_id: "Category",
  position: "Position",
  slowmode_seconds: "Slow mode",
  nickname: "Nickname",
  role: "Role",
  timed_out_until: "Timed out until",
  owner_id: "Owner",
  color: "Color",
  permissions: "Permissions",
  hoist: "Shown apart",
  mentionable: "Anyone can mention it",
  max_uses: "How many people",
  expires_at: "Expires",
  uses: "People it let in",
  min_account_age_seconds: "Minimum account age",
  applications: "Apply to join",
  linked_only: "waifu.dev accounts only",
  rules: "Rules",
  questions: "Questions",
  enabled: "On",
  channels: "Channels",
  keywords: "Words",
  allowed: "Allowed",
  mention_limit: "Ping limit",
  actions: "Actions",
};

/** Entries for things made or removed show just the one side of each change. */
const ONE_SIDE: Partial<Record<AuditAction, "before" | "after">> = {
  [AuditAction.INVITE_CREATE]: "after",
  [AuditAction.INVITE_DELETE]: "before",
  [AuditAction.AUTO_MOD_RULE_CREATE]: "after",
  [AuditAction.AUTO_MOD_RULE_DELETE]: "before",
  [AuditAction.EMOJI_CREATE]: "after",
  [AuditAction.EMOJI_DELETE]: "before",
};

/** Ranks from before roles, as entries from back then keep them. */
const OLD_RANK: Record<string, string> = { "1": "Member", "2": "Admin", "3": "Owner" };

/** Everything people did with their permissions, newest first, with who and what to filter by. */
export function AuditLog({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const inst = useInstance(instanceKey);
  const channels = inst?.channels[serverId];
  const roles = useRoles(instanceKey, serverId);
  const [entries, setEntries] = useState<AuditEntry[] | null>(null);
  const [users, setUsers] = useState<Record<string, User>>({});
  const [more, setMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [actor, setActor] = useState("");
  const [action, setAction] = useState<AuditAction>(AuditAction.UNSPECIFIED);
  const [open, setOpen] = useState<string | null>(null);

  const load = (filter: AuditFilter, append: boolean) => {
    setLoading(true);
    run(listAuditLog(instanceKey, serverId, filter))
      .then(
        (res) => {
          setEntries((list) => (append && list ? [...list, ...res.entries] : res.entries));
          setUsers((known) => ({ ...known, ...Object.fromEntries(res.users.map((u) => [u.id, u])) }));
          setMore(res.hasMore);
          setError(null);
        },
        (e: FuwaError) => setError(e.message),
      )
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    setEntries(null);
    load({ actorId: actor, action }, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [instanceKey, serverId, actor, action]);

  // The people who can show up as actors: the server's owner, anyone with a role, and anyone already seen doing something.
  const ownerId = inst?.servers.find((s) => s.id === serverId)?.ownerId;
  const actors = useMemo(() => {
    const managers = (inst?.members[serverId] ?? []).filter((m) => m.roleIds.length || m.user?.id === ownerId).map((m) => m.user!);
    const seen = new Map(managers.map((u) => [u.id, u]));
    for (const e of entries ?? []) if (users[e.actorId]) seen.set(e.actorId, users[e.actorId]!);
    return [...seen.values()];
  }, [inst?.members, serverId, entries, users, ownerId]);

  const actorLabel = actor ? displayName(users[actor] ?? actors.find((u) => u.id === actor)) : "Anyone";

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap gap-2">
        <Filter label="By" value={actorLabel}>
          <DropdownMenuRadioGroup value={actor} onValueChange={setActor}>
            <DropdownMenuRadioItem value="">Anyone</DropdownMenuRadioItem>
            {actors.map((u) => (
              <DropdownMenuRadioItem key={u.id} value={u.id}>
                <UserAvatar user={u} className="size-5" /> {displayName(u)}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </Filter>
        <Filter label="What" value={KINDS[action].label}>
          <DropdownMenuRadioGroup value={String(action)} onValueChange={(v) => setAction(Number(v) as AuditAction)}>
            {Object.entries(KINDS).map(([value, kind]) => (
              <DropdownMenuRadioItem key={value} value={value}>
                <kind.icon /> {kind.label}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </Filter>
      </div>

      {error && <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>}
      {!entries && !error && <div className="flex flex-col gap-2">{[0, 1, 2, 3].map((n) => <div key={n} className="shimmer h-14 rounded-2xl" />)}</div>}
      {entries && !entries.length && (
        <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} className="flex flex-col items-center gap-2 py-12 text-center">
          <ScrollTextIcon className="size-8 text-muted-foreground" />
          <p className="font-extrabold">Nothing here yet</p>
          <p className="text-sm text-muted-foreground">
            {actor || action ? "Nothing matches these filters." : "Changes to settings, channels and members show up here."}
          </p>
        </motion.div>
      )}
      {entries && entries.length > 0 && (
        <ol className="relative flex flex-col gap-1.5">
          <AnimatePresence initial={false}>
            {entries.map((entry, n) => (
              <Entry
                key={entry.id}
                entry={entry}
                index={n}
                users={users}
                channels={channels ?? []}
                roles={roles}
                open={open === entry.id}
                onToggle={() => setOpen(open === entry.id ? null : entry.id)}
              />
            ))}
          </AnimatePresence>
        </ol>
      )}
      {more && entries && (
        <Button
          type="button"
          variant="outline"
          disabled={loading}
          onClick={() => load({ actorId: actor, action, beforeId: entries[entries.length - 1]!.id }, true)}
          className="self-center rounded-xl"
        >
          {loading && <LoaderCircleIcon className="animate-spin" />} Show older
        </Button>
      )}
    </div>
  );
}

function Filter({ label, value, children }: { label: string; value: string; children: ReactNode }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="group flex h-10 items-center gap-2 rounded-xl border px-3 text-sm transition hover:border-primary/40 data-[state=open]:border-primary/60"
        >
          <span className="text-muted-foreground">{label}</span>
          <span className="max-w-40 truncate font-bold">{value}</span>
          <ChevronDownIcon className="size-4 text-muted-foreground transition-transform duration-300 group-data-[state=open]:rotate-180" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-80 w-60 overflow-y-auto">
        {children}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function Entry({
  entry,
  index,
  users,
  channels,
  roles,
  open,
  onToggle,
}: {
  entry: AuditEntry;
  index: number;
  users: Record<string, User>;
  channels: Channel[];
  roles: Role[];
  open: boolean;
  onToggle: () => void;
}) {
  const kind = KINDS[entry.action] ?? KINDS[AuditAction.UNSPECIFIED];
  const actor = users[entry.actorId];
  const at = toDate(entry.createdAt);
  const details = entry.changes.filter((c) => c.field !== "deleted_messages");
  const expandable = details.length > 0 || !!entry.reason;
  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, x: -12 }}
      animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: Math.min(index, 14) * 0.025 } }}
      exit={{ opacity: 0 }}
      className={cn("overflow-hidden rounded-2xl border bg-background/40 transition-colors", open && "border-primary/40 bg-muted/30")}
    >
      <button type="button" onClick={onToggle} disabled={!expandable} className="group flex w-full items-center gap-3 p-3 text-left">
        <span className={cn("relative grid size-9 shrink-0 place-items-center rounded-xl transition-transform duration-300 group-hover:scale-110", kind.tint)}>
          <kind.icon className="size-4" />
          <UserAvatar user={actor} className="absolute -right-1.5 -bottom-1.5 size-5 ring-2 ring-background" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block text-sm break-words">{sentence(entry, users, channels, roles)}</span>
          <span className="block text-xs text-muted-foreground" title={formatStamp(at)}>
            {formatStamp(at)}
          </span>
        </span>
        {expandable && (
          <ChevronDownIcon className={cn("size-4 shrink-0 text-muted-foreground transition-transform duration-300", open && "rotate-180")} />
        )}
      </button>
      <AnimatePresence initial={false}>
        {open && expandable && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.25, ease: [0.22, 1, 0.36, 1] }}
            className="overflow-hidden"
          >
            <div className="flex flex-col gap-1.5 border-t px-3 py-2.5 pl-15 text-sm">
              {entry.reason && (
                <p>
                  <span className="text-muted-foreground">Reason: </span>
                  {entry.reason}
                </p>
              )}
              {details.map((change, n) => (
                <motion.p
                  key={change.field}
                  initial={{ opacity: 0, y: 4 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ delay: n * 0.04 }}
                  className="flex flex-wrap items-center gap-1.5"
                >
                  <span className="text-muted-foreground">{FIELD[change.field] ?? change.field}:</span>
                  {ONE_SIDE[entry.action] ? (
                    <span className="rounded-md bg-muted px-1.5">{value(change.field, change[ONE_SIDE[entry.action]!], users, channels, entry)}</span>
                  ) : (
                    <>
                      <span className="rounded-md bg-destructive/10 px-1.5 text-destructive line-through decoration-destructive/50">
                        {value(change.field, change.before, users, channels, entry)}
                      </span>
                      <ArrowRightIcon className="size-3.5 text-muted-foreground" />
                      <span className="rounded-md bg-emerald-500/10 px-1.5 text-emerald-600 dark:text-emerald-400">
                        {value(change.field, change.after, users, channels, entry)}
                      </span>
                    </>
                  )}
                </motion.p>
              ))}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.li>
  );
}

/** A value from the log, in words. */
function value(field: string, raw: string, users: Record<string, User>, channels: Channel[], entry: AuditEntry): string {
  if (field === "discoverable" || field === "enabled") return raw === "true" ? "Yes" : "No";
  if (field === "role") return OLD_RANK[raw] ?? (raw ? entry.roleName : "None");
  if (field === "hoist" || field === "mentionable") return raw === "true" ? "Yes" : "No";
  if (field === "color") return raw || "None";
  if (field === "permissions") {
    const names = raw
      .split(",")
      .filter(Boolean)
      .map((n) => permissionLabel(Number(n) as Permission));
    return names.length ? names.join(", ") : "None";
  }
  if (field === "default_notifications")
    return Number(raw) === NotificationLevel.MENTIONS ? "Only @mentions" : Number(raw) === NotificationLevel.ALL ? "All messages" : "Each person's own";
  if (field === "system_channel_id" || field === "parent_id") {
    if (!raw) return "None";
    const channel = channels.find((c) => c.id === raw);
    return channel ? (field === "parent_id" ? channel.name : `#${channel.name}`) : "A deleted channel";
  }
  if (field === "slowmode_seconds") return raw === "0" ? "Off" : formatDuration(Number(raw));
  if (field === "timed_out_until") {
    if (!raw) return "Not timed out";
    const until = Number(raw);
    return formatStamp(new Date(until)) + ` (${formatDuration(Math.round((until - toDate(entry.createdAt).getTime()) / 1000))})`;
  }
  if (field === "owner_id") return displayName(users[raw]);
  if (field === "max_uses") return raw === "0" ? "No limit" : raw;
  if (field === "expires_at") return raw ? formatStamp(new Date(Number(raw))) : "Never";
  if (field === "min_account_age_seconds") return raw === "0" ? "Any age" : formatDuration(Number(raw));
  return raw || "Nothing";
}

function sentence(entry: AuditEntry, users: Record<string, User>, channels: Channel[], roles: Role[]): ReactNode {
  const actor = <b>{displayName(users[entry.actorId])}</b>;
  const change = (field: string) => entry.changes.find((c: AuditChange) => c.field === field);
  // Roles go by their name now, like channels, and the one they had once they're gone.
  const given = change("role");
  const roleId = entry.action === AuditAction.MEMBER_ROLES_UPDATE ? given?.after || given?.before : entry.targetId;
  const current = roles.find((r) => r.id === roleId);
  const tint = current?.color !== undefined ? { color: cssColor(current.color) } : undefined;
  const role = <b style={tint}>{current?.name ?? (entry.roleName || "a role")}</b>;
  const target = <b>{displayName(users[entry.targetId])}</b>;
  const known = channels.find((c) => c.id === entry.targetId);
  const channel = <b>{known ? (known.type === ChannelType.CATEGORY ? known.name : `#${known.name}`) : `#${entry.channelName}`}</b>;
  switch (entry.action) {
    case AuditAction.SERVER_UPDATE: {
      const listed = change("discoverable");
      const age = change("min_account_age_seconds");
      const apply = change("applications");
      const linked = change("linked_only");
      if (apply && entry.changes.length === 1)
        return apply.after === "true" ? <>{actor} made people apply to join</> : <>{actor} let people join without applying</>;
      if (linked && entry.changes.length === 1)
        return linked.after === "true" ? <>{actor} let in waifu.dev accounts only</> : <>{actor} let in accounts made on this instance too</>;
      if (listed && entry.changes.length === 1)
        return listed.after === "true" ? <>{actor} listed the server in Browse</> : <>{actor} made the server invite only</>;
      if (age && entry.changes.length === 1)
        return age.after === "0" ? (
          <>{actor} let in accounts of any age</>
        ) : (
          <>
            {actor} let in accounts once they're {formatDuration(Number(age.after))} old
          </>
        );
      return <>{actor} changed the server's settings</>;
    }
    case AuditAction.CHANNEL_CREATE:
      return <>{actor} created {channel}</>;
    case AuditAction.CHANNEL_UPDATE: {
      const slow = change("slowmode_seconds");
      if (slow && entry.changes.length === 1)
        return slow.after === "0" ? <>{actor} turned off slow mode in {channel}</> : <>{actor} set slow mode in {channel} to {formatDuration(Number(slow.after))}</>;
      return <>{actor} changed {channel}</>;
    }
    case AuditAction.CHANNEL_DELETE:
      return <>{actor} deleted <b>#{entry.channelName}</b></>;
    case AuditAction.CHANNELS_REORDER:
      return <>{actor} rearranged the channels</>;
    case AuditAction.MEMBER_UPDATE: {
      const rank = change("role");
      if (rank && entry.changes.length === 1)
        return rank.after === "2" ? <>{actor} made {target} an admin</> : <>{actor} made {target} a member again</>;
      return <>{actor} changed {target}'s nickname</>;
    }
    case AuditAction.MEMBER_ROLES_UPDATE:
      return given?.after ? (
        <>
          {actor} gave {target} {role}
        </>
      ) : (
        <>
          {actor} took {role} from {target}
        </>
      );
    case AuditAction.ROLE_CREATE:
      return <>{actor} created the role {role}</>;
    case AuditAction.ROLE_UPDATE: {
      const renamed = change("name");
      if (renamed && entry.changes.length === 1)
        return (
          <>
            {actor} renamed <b style={tint}>{renamed.before}</b> to <b style={tint}>{renamed.after}</b>
          </>
        );
      if (change("permissions") && entry.changes.length === 1) return <>{actor} changed what {role} can do</>;
      return <>{actor} changed {role}</>;
    }
    case AuditAction.ROLE_DELETE:
      return <>{actor} deleted the role {role}</>;
    case AuditAction.ROLES_REORDER:
      return <>{actor} rearranged the roles</>;
    case AuditAction.CHANNEL_PERMISSIONS_UPDATE:
      return <>{actor} changed who can do what in {channel}</>;
    case AuditAction.MEMBER_TIME_OUT: {
      const until = change("timed_out_until");
      if (!until?.after) return <>{actor} ended {target}'s time-out</>;
      const seconds = Math.round((Number(until.after) - toDate(entry.createdAt).getTime()) / 1000);
      return <>{actor} timed out {target} for {formatDuration(seconds)}</>;
    }
    case AuditAction.MEMBER_KICK:
      return <>{actor} kicked {target}</>;
    case AuditAction.MEMBER_BAN: {
      const deleted = Number(change("deleted_messages")?.after ?? 0);
      return (
        <>
          {actor} banned {target}
          {deleted > 0 && ` and deleted ${deleted} ${deleted === 1 ? "message" : "messages"}`}
        </>
      );
    }
    case AuditAction.MEMBER_UNBAN:
      return <>{actor} unbanned {target}</>;
    case AuditAction.MESSAGE_DELETE:
      return (
        <>
          {actor} deleted a message by {target} in <b>#{entry.channelName}</b>
        </>
      );
    case AuditAction.OWNERSHIP_TRANSFER:
      return <>{actor} handed the server to {target}</>;
    case AuditAction.INVITE_CREATE:
      return entry.channelName ? (
        <>
          {actor} made an invite to <b>#{entry.channelName}</b>
        </>
      ) : (
        <>{actor} made an invite</>
      );
    case AuditAction.INVITE_DELETE:
      return entry.channelName ? (
        <>
          {actor} revoked an invite to <b>#{entry.channelName}</b>
        </>
      ) : (
        <>{actor} revoked an invite</>
      );
    case AuditAction.APPLICATION_APPROVE:
      return <>{actor} let {target} in</>;
    case AuditAction.APPLICATION_REJECT:
      return <>{actor} turned down {target}'s application</>;
    case AuditAction.JOIN_FORM_UPDATE: {
      const rules = change("rules");
      const questions = change("questions");
      if (rules && !questions) return <>{actor} changed the rules</>;
      if (questions && !rules) return <>{actor} changed the questions</>;
      return <>{actor} changed the rules and questions</>;
    }
    case AuditAction.WELCOME_SCREEN_UPDATE: {
      const on = change("enabled");
      if (on && entry.changes.length === 1)
        return on.after === "true" ? <>{actor} turned on the welcome screen</> : <>{actor} turned off the welcome screen</>;
      return <>{actor} changed the welcome screen</>;
    }
    case AuditAction.AUTO_MOD_RULE_CREATE:
      return <>{actor} added the AutoMod rule <b>{change("name")?.after}</b></>;
    case AuditAction.AUTO_MOD_RULE_UPDATE: {
      const on = change("enabled");
      const name = <b>{change("name")?.after || "a rule"}</b>;
      if (on && entry.changes.filter((c) => c.field !== "name").length === 1)
        return on.after === "true" ? <>{actor} turned on the AutoMod rule {name}</> : <>{actor} paused the AutoMod rule {name}</>;
      return <>{actor} changed the AutoMod rule {name}</>;
    }
    case AuditAction.AUTO_MOD_RULE_DELETE:
      return <>{actor} deleted the AutoMod rule <b>{change("name")?.before}</b></>;
    case AuditAction.AUTO_MOD_TIME_OUT: {
      const until = change("timed_out_until");
      const seconds = until ? Math.round((Number(until.after) - toDate(entry.createdAt).getTime()) / 1000) : 0;
      return (
        <>
          <b>AutoMod</b> timed out {target}
          {seconds > 0 && ` for ${formatDuration(seconds)}`}
          {entry.channelName && (
            <>
              {" "}
              in <b>#{entry.channelName}</b>
            </>
          )}
        </>
      );
    }
    case AuditAction.EMOJI_CREATE:
      return <>{actor} added the emoji <b>:{change("name")?.after}:</b></>;
    case AuditAction.EMOJI_UPDATE: {
      const renamed = change("name");
      return (
        <>
          {actor} renamed <b>:{renamed?.before}:</b> to <b>:{renamed?.after}:</b>
        </>
      );
    }
    case AuditAction.EMOJI_DELETE:
      return <>{actor} deleted the emoji <b>:{change("name")?.before}:</b></>;
    default:
      return <>{actor} did something</>;
  }
}
