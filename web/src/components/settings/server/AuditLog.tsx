import {
  ArrowDownUpIcon,
  PinIcon,
  PinOffIcon,
  BarChart3Icon,
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
  LockOpenIcon,
  MessagesSquareIcon,
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
  WebhookIcon,
  UnplugIcon,
  KeyRoundIcon,
  KeySquareIcon,
  SendIcon,
  HandshakeIcon,
  SlidersHorizontalIcon,
  BanIcon,
  type LucideIcon,
} from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { AuditAction, type AuditChange, type AuditEntry } from "@/gen/fuwa/v1/server_pb";
import { ChannelType, NotificationLevel, type Channel, type Permission, type Role, type User } from "@/gen/fuwa/v1/types_pb";
import { listAuditLog, run, type AuditFilter } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance, useRoles } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { displayName, formatDuration, formatStamp, type Lang, toDate } from "@/lib/format";
import { type Key, T, useI18n } from "@/i18n/react";
import { cssColor, permissionLabel } from "@/lib/permissions";
import { cn } from "@/lib/utils";

type Kind = { label: Key; icon: LucideIcon; tint: string };

const KINDS: Record<AuditAction, Kind> = {
  [AuditAction.UNSPECIFIED]: { label: "serversettings.audit.kind.anything", icon: ScrollTextIcon, tint: "bg-muted text-muted-foreground" },
  [AuditAction.SERVER_UPDATE]: { label: "serversettings.audit.kind.serverUpdate", icon: SettingsIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.CHANNEL_CREATE]: { label: "serversettings.audit.kind.channelCreate", icon: FolderPlusIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.CHANNEL_UPDATE]: { label: "serversettings.audit.kind.channelUpdate", icon: HashIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.CHANNEL_DELETE]: { label: "serversettings.audit.kind.channelDelete", icon: Trash2Icon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.CHANNELS_REORDER]: { label: "serversettings.audit.kind.channelsReorder", icon: ArrowDownUpIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.CHANNEL_PERMISSIONS_UPDATE]: { label: "serversettings.audit.kind.channelPermissions", icon: LockIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.ROLE_CREATE]: { label: "serversettings.audit.kind.roleCreate", icon: ShieldPlusIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.ROLE_UPDATE]: { label: "serversettings.audit.kind.roleUpdate", icon: ShieldIcon, tint: "bg-violet-500/15 text-violet-500" },
  [AuditAction.ROLE_DELETE]: { label: "serversettings.audit.kind.roleDelete", icon: ShieldXIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.ROLES_REORDER]: { label: "serversettings.audit.kind.rolesReorder", icon: ArrowDownUpIcon, tint: "bg-violet-500/15 text-violet-500" },
  [AuditAction.MEMBER_ROLES_UPDATE]: { label: "serversettings.audit.kind.memberRoles", icon: UserCogIcon, tint: "bg-violet-500/15 text-violet-500" },
  [AuditAction.MEMBER_UPDATE]: { label: "serversettings.audit.kind.memberUpdate", icon: UserCogIcon, tint: "bg-violet-500/15 text-violet-500" },
  [AuditAction.MEMBER_TIME_OUT]: { label: "serversettings.audit.kind.timeOut", icon: HourglassIcon, tint: "bg-amber-500/15 text-amber-500" },
  [AuditAction.MEMBER_KICK]: { label: "serversettings.audit.kind.kick", icon: DoorOpenIcon, tint: "bg-orange-500/15 text-orange-500" },
  [AuditAction.MEMBER_BAN]: { label: "serversettings.audit.kind.ban", icon: GavelIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.MEMBER_UNBAN]: { label: "serversettings.audit.kind.unban", icon: UndoIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.MESSAGE_DELETE]: { label: "serversettings.audit.kind.messageDelete", icon: MessageSquareXIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.OWNERSHIP_TRANSFER]: { label: "serversettings.audit.kind.ownership", icon: CrownIcon, tint: "bg-amber-500/15 text-amber-500" },
  [AuditAction.INVITE_CREATE]: { label: "serversettings.audit.kind.inviteCreate", icon: LinkIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.INVITE_DELETE]: { label: "serversettings.audit.kind.inviteDelete", icon: Link2OffIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.APPLICATION_APPROVE]: { label: "serversettings.audit.kind.applicationApprove", icon: UserCheckIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.APPLICATION_REJECT]: { label: "serversettings.audit.kind.applicationReject", icon: UserXIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.JOIN_FORM_UPDATE]: { label: "serversettings.audit.kind.joinForm", icon: ClipboardListIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.WELCOME_SCREEN_UPDATE]: { label: "serversettings.audit.kind.welcome", icon: PartyPopperIcon, tint: "bg-pink-500/15 text-pink-500" },
  [AuditAction.AUTO_MOD_RULE_CREATE]: { label: "serversettings.audit.kind.automodCreate", icon: ShieldCheckIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.AUTO_MOD_RULE_UPDATE]: { label: "serversettings.audit.kind.automodUpdate", icon: ShieldAlertIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.AUTO_MOD_RULE_DELETE]: { label: "serversettings.audit.kind.automodDelete", icon: ShieldXIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.AUTO_MOD_TIME_OUT]: { label: "serversettings.audit.kind.automodTimeOut", icon: BotIcon, tint: "bg-amber-500/15 text-amber-500" },
  [AuditAction.AUTO_MOD_MESSAGE_DELETE]: { label: "serversettings.audit.kind.automodTakedown", icon: BotIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.EMOJI_CREATE]: { label: "serversettings.audit.kind.emojiCreate", icon: SmilePlusIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.EMOJI_UPDATE]: { label: "serversettings.audit.kind.emojiUpdate", icon: SmileIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.EMOJI_DELETE]: { label: "serversettings.audit.kind.emojiDelete", icon: FrownIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.WEBHOOK_CREATE]: { label: "serversettings.audit.kind.webhookCreate", icon: WebhookIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.WEBHOOK_UPDATE]: { label: "serversettings.audit.kind.webhookUpdate", icon: WebhookIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.WEBHOOK_DELETE]: { label: "serversettings.audit.kind.webhookDelete", icon: UnplugIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.AGENT_ADD]: { label: "serversettings.audit.kind.agentAdd", icon: BotIcon, tint: "bg-violet-500/15 text-violet-500" },
  [AuditAction.SHARE_CODE_CREATE]: { label: "serversettings.audit.kind.shareCodeCreate", icon: KeyRoundIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.SHARE_CODE_DELETE]: { label: "serversettings.audit.kind.shareCodeDelete", icon: KeySquareIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.SHARED_CHANNEL_REQUEST]: { label: "serversettings.audit.kind.sharedRequest", icon: SendIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.SHARED_CHANNEL_APPROVE]: { label: "serversettings.audit.kind.sharedApprove", icon: HandshakeIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.SHARED_CHANNEL_DISCONNECT]: { label: "serversettings.audit.kind.sharedDisconnect", icon: UnplugIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.SHARED_CHANNEL_UPDATE]: { label: "serversettings.audit.kind.sharedUpdate", icon: SlidersHorizontalIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.SHARED_CHANNEL_BLOCK]: { label: "serversettings.audit.kind.sharedBlock", icon: BanIcon, tint: "bg-orange-500/15 text-orange-500" },
  [AuditAction.SHARED_CHANNEL_UNBLOCK]: { label: "serversettings.audit.kind.sharedUnblock", icon: UndoIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.THREAD_LOCK]: { label: "serversettings.audit.kind.threadLock", icon: LockIcon, tint: "bg-amber-500/15 text-amber-500" },
  [AuditAction.THREAD_UNLOCK]: { label: "serversettings.audit.kind.threadUnlock", icon: LockOpenIcon, tint: "bg-emerald-500/15 text-emerald-500" },
  [AuditAction.THREAD_DELETE]: { label: "serversettings.audit.kind.threadDelete", icon: MessagesSquareIcon, tint: "bg-destructive/15 text-destructive" },
  [AuditAction.POLL_END]: { label: "serversettings.audit.kind.pollEnd", icon: BarChart3Icon, tint: "bg-amber-500/15 text-amber-500" },
  [AuditAction.ONBOARDING_UPDATE]: { label: "serversettings.audit.kind.onboarding", icon: PartyPopperIcon, tint: "bg-pink-500/15 text-pink-500" },
  [AuditAction.MESSAGE_PIN]: { label: "serversettings.audit.kind.messagePin", icon: PinIcon, tint: "bg-sky-500/15 text-sky-500" },
  [AuditAction.MESSAGE_UNPIN]: { label: "serversettings.audit.kind.messageUnpin", icon: PinOffIcon, tint: "bg-muted text-muted-foreground" },
};

/** What each changed field is called; catalog keys. Fields this app doesn't know show as they are. */
const FIELD: Record<string, Key> = {
  name: "serversettings.overview.name",
  description: "serversettings.nav.description",
  icon_url: "serversettings.audit.field.icon",
  discoverable: "serversettings.audit.field.discoverable",
  default_notifications: "serversettings.nav.defaultNotifications",
  system_channel_id: "serversettings.nav.joinMessages",
  topic: "serversettings.channels.topic",
  encryption: "serversettings.audit.field.encryption",
  share_history: "serversettings.audit.field.shareHistory",
  parent_id: "serversettings.channels.category",
  position: "serversettings.audit.field.position",
  slowmode_seconds: "serversettings.nav.slowmode",
  nickname: "settings.nav.nickname",
  role: "serversettings.audit.field.role",
  timed_out_until: "serversettings.audit.field.timedOutUntil",
  owner_id: "serversettings.shared.owner",
  color: "serversettings.audit.field.color",
  permissions: "serversettings.shared.permissions",
  hoist: "serversettings.audit.field.hoist",
  mentionable: "serversettings.audit.field.mentionable",
  max_uses: "serversettings.audit.field.maxUses",
  expires_at: "serversettings.audit.field.expires",
  uses: "serversettings.audit.field.uses",
  min_account_age_seconds: "serversettings.nav.accountAge",
  thread_archive_hours: "serversettings.audit.field.threadArchive",
  record_video: "serversettings.audit.field.recordVideo",
  applications: "serversettings.nav.applyToJoin",
  linked_only: "serversettings.nav.linkedOnly",
  rules: "serversettings.nav.rules",
  avatar_url: "serversettings.audit.field.picture",
  channel_id: "serversettings.audit.field.postsIn",
  token: "serversettings.audit.field.address",
  questions: "serversettings.audit.field.questions",
  enabled: "serversettings.audit.field.enabled",
  channels: "serversettings.nav.channels",
  keywords: "serversettings.audit.field.keywords",
  allowed: "serversettings.audit.field.allowed",
  mention_limit: "serversettings.audit.field.mentionLimit",
  server: "serversettings.invites.server",
  actions: "serversettings.audit.field.actions",
};

/** A changed field's name in the app's language. */
const fieldName = (t: Lang["t"], field: string) => (Object.hasOwn(FIELD, field) ? t(FIELD[field]!) : field);

/** Entries for things made or removed show just the one side of each change. */
const ONE_SIDE: Partial<Record<AuditAction, "before" | "after">> = {
  [AuditAction.INVITE_CREATE]: "after",
  [AuditAction.INVITE_DELETE]: "before",
  [AuditAction.AUTO_MOD_RULE_CREATE]: "after",
  [AuditAction.AUTO_MOD_RULE_DELETE]: "before",
  [AuditAction.EMOJI_CREATE]: "after",
  [AuditAction.EMOJI_DELETE]: "before",
  [AuditAction.WEBHOOK_CREATE]: "after",
  [AuditAction.WEBHOOK_DELETE]: "before",
};

/** Ranks from before roles, as entries from back then keep them. */
const OLD_RANK: Record<string, Key> = { "1": "serversettings.audit.rank.member", "2": "serversettings.audit.rank.admin", "3": "serversettings.shared.owner" };

/** Everything people did with their permissions, newest first, with who and what to filter by. */
export function AuditLog({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
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

  const actorLabel = actor ? displayName(users[actor] ?? actors.find((u) => u.id === actor)) : t("serversettings.audit.anyone");

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap gap-2">
        <Filter label={t("serversettings.audit.by")} value={actorLabel}>
          <DropdownMenuRadioGroup value={actor} onValueChange={setActor}>
            <DropdownMenuRadioItem value="">{t("serversettings.audit.anyone")}</DropdownMenuRadioItem>
            {actors.map((u) => (
              <DropdownMenuRadioItem key={u.id} value={u.id}>
                <UserAvatar user={u} className="size-5" /> {displayName(u)}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </Filter>
        <Filter label={t("serversettings.audit.what")} value={t(KINDS[action].label)}>
          <DropdownMenuRadioGroup value={String(action)} onValueChange={(v) => setAction(Number(v) as AuditAction)}>
            {Object.entries(KINDS).map(([value, kind]) => (
              <DropdownMenuRadioItem key={value} value={value}>
                <kind.icon /> {t(kind.label)}
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
          <p className="font-extrabold">{t("serversettings.audit.empty")}</p>
          <p className="text-sm text-muted-foreground">{actor || action ? t("serversettings.audit.noMatch") : t("serversettings.audit.emptyHint")}</p>
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
          {loading && <LoaderCircleIcon className="animate-spin" />} {t("serversettings.audit.older")}
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
  const lang = useI18n();
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
      className={cn("relative overflow-hidden rounded-2xl border bg-background/40 transition-colors", open && "border-primary/40 bg-muted/30")}
    >
      <button type="button" onClick={onToggle} disabled={!expandable} className="group flex w-full items-center gap-3 p-3 text-left">
        <span className={cn("relative grid size-9 shrink-0 place-items-center rounded-xl transition-transform duration-300 group-hover:scale-110", kind.tint)}>
          <kind.icon className="size-4" />
          <UserAvatar user={actor} className="absolute -right-1.5 -bottom-1.5 size-5 ring-2 ring-background" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block text-sm break-words">{sentence(lang, entry, users, channels, roles)}</span>
          <span className="block text-xs text-muted-foreground" title={formatStamp(at)}>
            {formatStamp(at)}
          </span>
        </span>
        {expandable && (
          <ChevronDownIcon className={cn("size-4 shrink-0 text-muted-foreground transition-transform duration-300", open && "rotate-180")} />
        )}
      </button>
      <AnimatePresence initial={false} mode="popLayout">
        {open && expandable && (
          <motion.div {...SLIDE_IN} transition={{ duration: 0.25, ease: [0.22, 1, 0.36, 1] }}>
            <div className="flex flex-col gap-1.5 border-t px-3 py-2.5 pl-15 text-sm">
              {entry.reason && (
                <p>
                  <span className="text-muted-foreground">{lang.t("serversettings.audit.reason")} </span>
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
                  <span className="text-muted-foreground">{fieldName(lang.t, change.field)}:</span>
                  {ONE_SIDE[entry.action] ? (
                    <span className="rounded-md bg-muted px-1.5">{value(lang, change.field, change[ONE_SIDE[entry.action]!], users, channels, entry)}</span>
                  ) : (
                    <>
                      <span className="rounded-md bg-destructive/10 px-1.5 text-destructive line-through decoration-destructive/50">
                        {value(lang, change.field, change.before, users, channels, entry)}
                      </span>
                      <ArrowRightIcon className="size-3.5 text-muted-foreground" />
                      <span className="rounded-md bg-emerald-500/10 px-1.5 text-emerald-600 dark:text-emerald-400">
                        {value(lang, change.field, change.after, users, channels, entry)}
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
function value(lang: Lang, field: string, raw: string, users: Record<string, User>, channels: Channel[], entry: AuditEntry): string {
  const { t } = lang;
  const yesNo = () => (raw === "true" ? t("serversettings.audit.value.yes") : t("serversettings.audit.value.no"));
  const none = t("serversettings.audit.value.none");
  if (field === "discoverable" || field === "enabled") return yesNo();
  if (field === "role") return Object.hasOwn(OLD_RANK, raw) ? t(OLD_RANK[raw]!) : raw ? entry.roleName : none;
  if (field === "hoist" || field === "mentionable") return yesNo();
  if (field === "color") return raw || none;
  if (field === "permissions") {
    const names = raw
      .split(",")
      .filter(Boolean)
      .map((n) => permissionLabel(t, Number(n) as Permission));
    return names.length ? names.join(", ") : none;
  }
  if (field === "default_notifications")
    return Number(raw) === NotificationLevel.MENTIONS
      ? t("common.notify.mentions")
      : Number(raw) === NotificationLevel.ALL
        ? t("common.notify.all")
        : t("serversettings.audit.value.eachOwn");
  if (field === "avatar_url") return raw ? t("serversettings.audit.value.aPicture") : none;
  if (field === "token") return t("serversettings.audit.value.replaced");
  if (field === "system_channel_id" || field === "parent_id" || field === "channel_id") {
    if (!raw) return none;
    const channel = channels.find((c) => c.id === raw);
    return channel ? (field === "parent_id" ? channel.name : `#${channel.name}`) : t("serversettings.audit.value.deletedChannel");
  }
  if (field === "slowmode_seconds") return raw === "0" ? t("serversettings.shared.off") : formatDuration(lang, Number(raw));
  if (field === "timed_out_until") {
    if (!raw) return t("serversettings.audit.value.notTimedOut");
    const until = Number(raw);
    return t("serversettings.audit.value.until", {
      time: formatStamp(new Date(until)),
      duration: formatDuration(lang, Math.round((until - toDate(entry.createdAt).getTime()) / 1000)),
    });
  }
  if (field === "owner_id") return displayName(users[raw]);
  if (field === "max_uses") return raw === "0" ? t("settings.controls.noLimit") : raw;
  if (field === "expires_at") return raw ? formatStamp(new Date(Number(raw))) : t("serversettings.shared.never");
  if (field === "min_account_age_seconds") return raw === "0" ? t("serversettings.access.age.any") : formatDuration(lang, Number(raw));
  if (field === "thread_archive_hours") return raw === "0" ? t("serversettings.shared.never") : formatDuration(lang, Number(raw) * 3600);
  if (field === "record_video") return raw === "true" ? t("serversettings.audit.value.soundVideo") : t("serversettings.audit.value.soundOnly");
  return raw || t("serversettings.audit.value.nothing");
}

/** What an entry says happened: a sentence per kind of change, with the people, roles and channels in bold. */
function sentence(lang: Lang, entry: AuditEntry, users: Record<string, User>, channels: Channel[], roles: Role[]): ReactNode {
  const actor = <b>{displayName(users[entry.actorId])}</b>;
  const change = (field: string) => entry.changes.find((c: AuditChange) => c.field === field);
  // Roles go by their name now, like channels, and the one they had once they're gone.
  const given = change("role");
  const roleId = entry.action === AuditAction.MEMBER_ROLES_UPDATE ? given?.after || given?.before : entry.targetId;
  const current = roles.find((r) => r.id === roleId);
  const tint = current?.color !== undefined ? { color: cssColor(current.color) } : undefined;
  const role = <b style={tint}>{current?.name ?? (entry.roleName || lang.t("serversettings.audit.aRole"))}</b>;
  const target = <b>{displayName(users[entry.targetId])}</b>;
  const known = channels.find((c) => c.id === entry.targetId);
  const channel = <b>{known ? (known.type === ChannelType.CATEGORY ? known.name : `#${known.name}`) : `#${entry.channelName}`}</b>;
  // The channel by the name the entry kept, for things that may be gone now.
  const named = <b>#{entry.channelName}</b>;
  const automod = <b>AutoMod</b>;
  const say = (k: Key, values: Record<string, ReactNode> = {}, count?: number) => <T k={k} values={{ actor, ...values }} count={count} />;
  const seconds = (until: string) => Math.round((Number(until) - toDate(entry.createdAt).getTime()) / 1000);
  switch (entry.action) {
    case AuditAction.SERVER_UPDATE: {
      const listed = change("discoverable");
      const age = change("min_account_age_seconds");
      const apply = change("applications");
      const linked = change("linked_only");
      const video = change("record_video");
      const only = entry.changes.length === 1;
      if (video && only) return say(video.after === "true" ? "serversettings.audit.s.recordVideoOn" : "serversettings.audit.s.recordSoundOnly");
      if (apply && only) return say(apply.after === "true" ? "serversettings.audit.s.applyOn" : "serversettings.audit.s.applyOff");
      if (linked && only) return say(linked.after === "true" ? "serversettings.audit.s.linkedOnlyOn" : "serversettings.audit.s.linkedOnlyOff");
      if (listed && only) return say(listed.after === "true" ? "serversettings.audit.s.listed" : "serversettings.audit.s.inviteOnly");
      if (age && only)
        return age.after === "0"
          ? say("serversettings.audit.s.anyAge")
          : say("serversettings.audit.s.minAge", { duration: formatDuration(lang, Number(age.after)) });
      return say("serversettings.audit.s.serverUpdate");
    }
    case AuditAction.CHANNEL_CREATE:
      return say("serversettings.audit.s.channelCreate", { channel });
    case AuditAction.CHANNEL_UPDATE: {
      const slow = change("slowmode_seconds");
      if (slow && entry.changes.length === 1)
        return slow.after === "0"
          ? say("serversettings.audit.s.slowOff", { channel })
          : say("serversettings.audit.s.slowSet", { channel, duration: formatDuration(lang, Number(slow.after)) });
      return say("serversettings.audit.s.channelUpdate", { channel });
    }
    case AuditAction.CHANNEL_DELETE:
      return say("serversettings.audit.s.channelDelete", { channel: named });
    case AuditAction.CHANNELS_REORDER:
      return say("serversettings.audit.s.channelsReorder");
    case AuditAction.MEMBER_UPDATE: {
      const rank = change("role");
      if (rank && entry.changes.length === 1) return say(rank.after === "2" ? "serversettings.audit.s.madeAdmin" : "serversettings.audit.s.madeMember", { target });
      return say("serversettings.audit.s.nickname", { target });
    }
    case AuditAction.MEMBER_ROLES_UPDATE: {
      // Picked in the server's onboarding: role names, comma-separated.
      const picked = change("roles");
      if (picked)
        return picked.after
          ? say("serversettings.audit.s.picked", { target, roles: <b>{picked.after}</b> })
          : say("serversettings.audit.s.unpicked", { target, roles: <b>{picked.before}</b> });
      return say(given?.after ? "serversettings.audit.s.roleGive" : "serversettings.audit.s.roleTake", { target, role });
    }
    case AuditAction.ROLE_CREATE:
      return say("serversettings.audit.s.roleCreate", { role });
    case AuditAction.ROLE_UPDATE: {
      const renamed = change("name");
      if (renamed && entry.changes.length === 1)
        return say("serversettings.audit.s.renamed", { before: <b style={tint}>{renamed.before}</b>, after: <b style={tint}>{renamed.after}</b> });
      if (change("permissions") && entry.changes.length === 1) return say("serversettings.audit.s.rolePermissions", { role });
      return say("serversettings.audit.s.roleUpdate", { role });
    }
    case AuditAction.ROLE_DELETE:
      return say("serversettings.audit.s.roleDelete", { role });
    case AuditAction.ROLES_REORDER:
      return say("serversettings.audit.s.rolesReorder");
    case AuditAction.CHANNEL_PERMISSIONS_UPDATE:
      return say("serversettings.audit.s.channelPermissions", { channel });
    case AuditAction.MEMBER_TIME_OUT: {
      const until = change("timed_out_until");
      if (!until?.after) return say("serversettings.audit.s.timeOutEnd", { target });
      return say("serversettings.audit.s.timeOut", { target, duration: formatDuration(lang, seconds(until.after)) });
    }
    case AuditAction.MEMBER_KICK:
      return say("serversettings.audit.s.kick", { target });
    case AuditAction.MEMBER_BAN: {
      const deleted = Number(change("deleted_messages")?.after ?? 0);
      return deleted > 0 ? say("serversettings.audit.s.banDeleted", { target, count: deleted }, deleted) : say("serversettings.audit.s.ban", { target });
    }
    case AuditAction.MEMBER_UNBAN:
      return say("serversettings.audit.s.unban", { target });
    case AuditAction.MESSAGE_DELETE:
      return say("serversettings.audit.s.messageDelete", { target, channel: named });
    case AuditAction.OWNERSHIP_TRANSFER:
      return say("serversettings.audit.s.ownership", { target });
    case AuditAction.INVITE_CREATE:
      return entry.channelName ? say("serversettings.audit.s.inviteCreateIn", { channel: named }) : say("serversettings.audit.s.inviteCreate");
    case AuditAction.INVITE_DELETE:
      return entry.channelName ? say("serversettings.audit.s.inviteDeleteIn", { channel: named }) : say("serversettings.audit.s.inviteDelete");
    case AuditAction.APPLICATION_APPROVE:
      return say("serversettings.audit.s.applicationApprove", { target });
    case AuditAction.APPLICATION_REJECT:
      return say("serversettings.audit.s.applicationReject", { target });
    case AuditAction.JOIN_FORM_UPDATE: {
      const rules = change("rules");
      const questions = change("questions");
      if (rules && !questions) return say("serversettings.audit.s.rulesChanged");
      if (questions && !rules) return say("serversettings.audit.s.questionsChanged");
      return say("serversettings.audit.s.joinFormChanged");
    }
    case AuditAction.WELCOME_SCREEN_UPDATE: {
      const on = change("enabled");
      if (on && entry.changes.length === 1) return say(on.after === "true" ? "serversettings.audit.s.welcomeOn" : "serversettings.audit.s.welcomeOff");
      return say("serversettings.audit.s.welcomeChanged");
    }
    case AuditAction.ONBOARDING_UPDATE: {
      const on = change("enabled");
      if (on && entry.changes.length === 1) return say(on.after === "true" ? "serversettings.audit.s.onboardingOn" : "serversettings.audit.s.onboardingOff");
      return say("serversettings.audit.s.onboardingChanged");
    }
    case AuditAction.AUTO_MOD_RULE_CREATE:
      return say("serversettings.audit.s.automodCreate", { name: <b>{change("name")?.after}</b> });
    case AuditAction.AUTO_MOD_RULE_UPDATE: {
      const on = change("enabled");
      const name = <b>{change("name")?.after || lang.t("serversettings.audit.aRule")}</b>;
      if (on && entry.changes.filter((c) => c.field !== "name").length === 1)
        return say(on.after === "true" ? "serversettings.audit.s.automodOn" : "serversettings.audit.s.automodPaused", { name });
      return say("serversettings.audit.s.automodChanged", { name });
    }
    case AuditAction.AUTO_MOD_RULE_DELETE:
      return say("serversettings.audit.s.automodDelete", { name: <b>{change("name")?.before}</b> });
    case AuditAction.AUTO_MOD_TIME_OUT: {
      const until = change("timed_out_until");
      const length = until ? seconds(until.after) : 0;
      const duration = formatDuration(lang, length);
      if (length > 0)
        return entry.channelName
          ? say("serversettings.audit.s.automodTimeOutForIn", { automod, target, duration, channel: named })
          : say("serversettings.audit.s.automodTimeOutFor", { automod, target, duration });
      return entry.channelName
        ? say("serversettings.audit.s.automodTimeOutIn", { automod, target, channel: named })
        : say("serversettings.audit.s.automodTimeOut", { automod, target });
    }
    case AuditAction.AUTO_MOD_MESSAGE_DELETE:
      return entry.channelName
        ? say("serversettings.audit.s.automodTakedownIn", { automod, target, channel: named })
        : say("serversettings.audit.s.automodTakedown", { automod, target });
    case AuditAction.EMOJI_CREATE:
      return say("serversettings.audit.s.emojiCreate", { name: <b>:{change("name")?.after}:</b> });
    case AuditAction.EMOJI_UPDATE: {
      const renamed = change("name");
      return say("serversettings.audit.s.renamed", { before: <b>:{renamed?.before}:</b>, after: <b>:{renamed?.after}:</b> });
    }
    case AuditAction.EMOJI_DELETE:
      return say("serversettings.audit.s.emojiDelete", { name: <b>:{change("name")?.before}:</b> });
    case AuditAction.WEBHOOK_CREATE:
      return say("serversettings.audit.s.webhookCreate", { name: <b>{change("name")?.after}</b>, channel: named });
    case AuditAction.WEBHOOK_UPDATE: {
      if (change("token")) return say("serversettings.audit.s.webhookToken", { channel: named });
      const renamed = change("name");
      if (renamed && entry.changes.length === 1)
        return say("serversettings.audit.s.webhookRenamed", { before: <b>{renamed.before}</b>, after: <b>{renamed.after}</b> });
      return say("serversettings.audit.s.webhookUpdate", { channel: named });
    }
    case AuditAction.WEBHOOK_DELETE:
      return say("serversettings.audit.s.webhookDelete", { name: <b>{change("name")?.before}</b> });
    case AuditAction.AGENT_ADD:
      return say("serversettings.audit.s.agentAdd", { target });
    case AuditAction.SHARE_CODE_CREATE:
      return say("serversettings.audit.s.shareCodeCreate", { channel: named });
    case AuditAction.SHARE_CODE_DELETE:
      return say("serversettings.audit.s.shareCodeDelete", { channel: named });
    case AuditAction.SHARED_CHANNEL_REQUEST:
      return say("serversettings.audit.s.sharedRequest", { channel: named, server: <b>{change("server")?.after || lang.t("serversettings.audit.anotherServer")}</b> });
    case AuditAction.SHARED_CHANNEL_APPROVE:
      return say("serversettings.audit.s.sharedApprove", { channel: named, server: <b>{change("server")?.after || lang.t("serversettings.audit.anotherServer")}</b> });
    case AuditAction.SHARED_CHANNEL_DISCONNECT: {
      const other = change("server")?.after;
      return other
        ? say("serversettings.audit.s.sharedDisconnectWith", { channel: named, server: <b>{other}</b> })
        : say("serversettings.audit.s.sharedDisconnect", { channel: named });
    }
    case AuditAction.SHARED_CHANNEL_UPDATE:
      return say("serversettings.audit.s.sharedUpdate", { channel: named });
    case AuditAction.SHARED_CHANNEL_BLOCK:
      return say("serversettings.audit.s.sharedBlock", { target, channel: named });
    case AuditAction.SHARED_CHANNEL_UNBLOCK:
      return say("serversettings.audit.s.sharedUnblock", { target, channel: named });
    case AuditAction.POLL_END:
      return entry.channelName ? say("serversettings.audit.s.pollEndIn", { target, channel: named }) : say("serversettings.audit.s.pollEnd", { target });
    case AuditAction.THREAD_LOCK:
      return say("serversettings.audit.s.threadLock", { target, channel: named });
    case AuditAction.THREAD_UNLOCK:
      return say("serversettings.audit.s.threadUnlock", { target, channel: named });
    case AuditAction.THREAD_DELETE:
      return say("serversettings.audit.s.threadDelete", { target, channel: named });
    case AuditAction.MESSAGE_PIN:
      return say("serversettings.audit.s.messagePin", { target, channel: named });
    case AuditAction.MESSAGE_UNPIN:
      return say("serversettings.audit.s.messageUnpin", { target, channel: named });
    default:
      return say("serversettings.audit.s.unknown");
  }
}
