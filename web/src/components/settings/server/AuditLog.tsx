import {
  ArrowDownUpIcon,
  ArrowRightIcon,
  ChevronDownIcon,
  CrownIcon,
  DoorOpenIcon,
  FolderPlusIcon,
  GavelIcon,
  HashIcon,
  HourglassIcon,
  LoaderCircleIcon,
  MessageSquareXIcon,
  ScrollTextIcon,
  SettingsIcon,
  Trash2Icon,
  UndoIcon,
  UserCogIcon,
  type LucideIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { AuditAction, type AuditChange, type AuditEntry } from "@/gen/fuwa/v1/server_pb";
import { ChannelType, MemberRole, NotificationLevel, type Channel, type User } from "@/gen/fuwa/v1/types_pb";
import { listAuditLog, run, type AuditFilter } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
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
import { cn } from "@/lib/utils";

type Kind = { label: string; icon: LucideIcon; tint: string };

const KINDS: Record<AuditAction, Kind> = {
  [AuditAction.UNSPECIFIED]: { label: "Anything", icon: ScrollTextIcon, tint: "bg-muted text-muted-foreground" },
  [AuditAction.SERVER_UPDATE]: { label: "Server settings", icon: SettingsIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.CHANNEL_CREATE]: { label: "New channels", icon: FolderPlusIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.CHANNEL_UPDATE]: { label: "Channel changes", icon: HashIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.CHANNEL_DELETE]: { label: "Deleted channels", icon: Trash2Icon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.CHANNELS_REORDER]: { label: "Channel order", icon: ArrowDownUpIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.MEMBER_UPDATE]: { label: "Roles and nicknames", icon: UserCogIcon, tint: "bg-violet-500/15 text-violet-500" },
  [AuditAction.MEMBER_TIME_OUT]: { label: "Time-outs", icon: HourglassIcon, tint: "bg-amber-500/15 text-amber-500" },
  [AuditAction.MEMBER_KICK]: { label: "Kicks", icon: DoorOpenIcon, tint: "bg-orange-500/15 text-orange-500" },
  [AuditAction.MEMBER_BAN]: { label: "Bans", icon: GavelIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.MEMBER_UNBAN]: { label: "Unbans", icon: UndoIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.MESSAGE_DELETE]: { label: "Deleted messages", icon: MessageSquareXIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.OWNERSHIP_TRANSFER]: { label: "Ownership", icon: CrownIcon, tint: "bg-amber-500/15 text-amber-500" },
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
};

const ROLE: Record<string, string> = { [MemberRole.MEMBER]: "Member", [MemberRole.ADMIN]: "Admin", [MemberRole.OWNER]: "Owner" };

/** Everything owners and admins did, newest first, with who and what to filter by. */
export function AuditLog({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const inst = useInstance(instanceKey);
  const channels = inst?.channels[serverId];
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

  // The people who can show up as actors: the server's owner and admins, and anyone already seen doing something.
  const actors = useMemo(() => {
    const managers = (inst?.members[serverId] ?? []).filter((m) => m.role >= MemberRole.ADMIN).map((m) => m.user!);
    const seen = new Map(managers.map((u) => [u.id, u]));
    for (const e of entries ?? []) if (users[e.actorId]) seen.set(e.actorId, users[e.actorId]!);
    return [...seen.values()];
  }, [inst?.members, serverId, entries, users]);

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
  open,
  onToggle,
}: {
  entry: AuditEntry;
  index: number;
  users: Record<string, User>;
  channels: Channel[];
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
          <span className="block text-sm break-words">{sentence(entry, users, channels)}</span>
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
                  <span className="rounded-md bg-destructive/10 px-1.5 text-destructive line-through decoration-destructive/50">
                    {value(change.field, change.before, users, channels, entry)}
                  </span>
                  <ArrowRightIcon className="size-3.5 text-muted-foreground" />
                  <span className="rounded-md bg-emerald-500/10 px-1.5 text-emerald-600 dark:text-emerald-400">
                    {value(change.field, change.after, users, channels, entry)}
                  </span>
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
  if (field === "discoverable") return raw === "true" ? "Yes" : "No";
  if (field === "role") return ROLE[raw] ?? raw;
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
  return raw || "Nothing";
}

function sentence(entry: AuditEntry, users: Record<string, User>, channels: Channel[]): ReactNode {
  const actor = <b>{displayName(users[entry.actorId])}</b>;
  const target = <b>{displayName(users[entry.targetId])}</b>;
  const known = channels.find((c) => c.id === entry.targetId);
  const channel = <b>{known ? (known.type === ChannelType.CATEGORY ? known.name : `#${known.name}`) : `#${entry.channelName}`}</b>;
  const change = (field: string) => entry.changes.find((c: AuditChange) => c.field === field);
  switch (entry.action) {
    case AuditAction.SERVER_UPDATE:
      return <>{actor} changed the server's settings</>;
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
      const role = change("role");
      if (role && entry.changes.length === 1)
        return Number(role.after) === MemberRole.ADMIN ? <>{actor} made {target} an admin</> : <>{actor} made {target} a member again</>;
      return <>{actor} changed {target}'s nickname</>;
    }
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
    default:
      return <>{actor} did something</>;
  }
}
