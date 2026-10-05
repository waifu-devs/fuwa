import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { BellIcon, BellOffIcon, BellRingIcon, ChartColumnIcon, ChevronDownIcon, ChevronRightIcon, ClipboardListIcon, DoorOpenIcon, FingerprintIcon, HashIcon, IdCardIcon, LockIcon, LockKeyholeIcon, MegaphoneIcon, PartyPopperIcon, PlusIcon, ScrollTextIcon, SettingsIcon, ShieldCheckIcon, UserPlusIcon, Volume2Icon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useMemo, useRef, useState, type Ref } from "react";
import { ChannelType, Permission, type Channel } from "@/gen/fuwa/v1/types_pb";
import { leaveServer, listApplications, reorderChannels, run, updateNotifications } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess, useAction } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { CreateChannelDialog } from "@/components/dialogs/CreateChannelDialog";
import { InviteDialog } from "@/components/dialogs/InviteDialog";
import { useServerSettingsTabs } from "@/components/dialogs/serverSettingsTabs";
import { lazyComponent } from "@/components/lazy";
import { RulesDialog } from "@/components/join/Rules";
import { WelcomeGate } from "@/components/join/Welcome";
import { useLayout } from "@/components/Shell";
import { useSsoLocked } from "@/components/join/SsoGate";
import { Count, SPRING, SwapText } from "@/components/motion";
import { Private } from "@/components/Private";
import { UserPanel } from "@/components/UserPanel";
import { CallPanel } from "@/components/calls/CallPanel";
import { SharedBadge } from "@/components/chat/Shared";
import { VoiceUsers } from "@/components/calls/VoiceUsers";
import { joinCall } from "@/calls/engine";
import { useContextMenu } from "@/components/ContextMenu";
import { categoryMenu, channelMenu, type ChannelMenuActions } from "@/components/menus/channel";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useArrange } from "@/hooks/use-arrange";
import { layoutOf, placements } from "@/lib/arrange";
import { T, useI18n } from "@/i18n/react";
import { isMuted, MUTE_FOR, muteForLabel, mutedHint, useMuted, useNotificationSettings, useNow } from "@/lib/notifications";
import { has, hasIn, isPrivate } from "@/lib/permissions";
import { usePrefs } from "@/lib/prefs";
import { copy, openSettings, toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const ServerSettingsDialog = lazyComponent(
  () => import("@/components/dialogs/ServerSettingsDialog").then((m) => m.ServerSettingsDialog),
  (p) => p.open,
);

export const CHANNEL_ICON: Partial<Record<ChannelType, typeof HashIcon>> = {
  [ChannelType.ANNOUNCEMENT]: MegaphoneIcon,
  [ChannelType.VOICE]: Volume2Icon,
  [ChannelType.SECURE]: ShieldCheckIcon,
};

type Group = { category: Channel | null; channels: Channel[] };

/** Channels you can open, in the order the sidebar lists them. */
export const openableChannels = (channels: Channel[]) =>
  groupChannels(channels)
    .flatMap((g) => g.channels)
    .filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT || c.type === ChannelType.SECURE);

export function groupChannels(channels: Channel[]): Group[] {
  const categories = channels.filter((c) => c.type === ChannelType.CATEGORY);
  const known = new Set(categories.map((c) => c.id));
  const loose = channels.filter((c) => c.type !== ChannelType.CATEGORY && !known.has(c.parentId));
  return [
    { category: null, channels: loose },
    ...categories.map((category) => ({
      category,
      channels: channels.filter((c) => c.parentId === category.id && c.type !== ChannelType.CATEGORY),
    })),
  ];
}

/** Mute a server from its menu, or open its notification settings. */
function ServerNotificationItems({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const now = useNow();
  const settings = useNotificationSettings(instanceKey, serverId);
  const mute = (mutedUntil: Date | null | false) =>
    run(updateNotifications(instanceKey, serverId, "", { mutedUntil })).catch((err: FuwaError) => toast(err.message));
  return (
    <>
      {isMuted(settings, now) ? (
        <DropdownMenuItem onSelect={() => void mute(false)}>
          <BellIcon /> {t("workspace.sidebar.unmuteServer")}
          <span className="ml-auto truncate pl-2 text-xs text-muted-foreground">{mutedHint(t, settings, now)}</span>
        </DropdownMenuItem>
      ) : (
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <BellOffIcon /> {t("workspace.sidebar.muteServer")}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-52">
            {MUTE_FOR.map((m) => (
              <DropdownMenuItem key={m.id} onSelect={() => void mute(m.ms === null ? null : new Date(Date.now() + m.ms))}>
                {muteForLabel(t, m)}
              </DropdownMenuItem>
            ))}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
      )}
      <DropdownMenuItem onSelect={() => openSettings("server-notifications", serverId)}>
        <BellRingIcon /> {t("workspace.sidebar.notificationSettings")}
      </DropdownMenuItem>
    </>
  );
}

export function ChannelSidebar({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  // Only what the sidebar shows, so messages and member changes elsewhere don't re-render it.
  const known = useFuwa((s) => !!s.instances[instanceKey]);
  const nodeName = useFuwa((s) => s.instances[instanceKey]?.node?.name);
  const params = useParams({ strict: false }) as { channel?: string };
  const navigate = useNavigate();
  const server = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId));
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[serverId]);
  const synced = useFuwa((s) => s.instances[instanceKey]?.synced[serverId]);
  const access = useAccess(instanceKey, serverId);
  const owner = access.owner;
  const settingsTabs = useServerSettingsTabs(instanceKey, serverId);
  const canCreate = has(access, Permission.MANAGE_CHANNELS);
  const usage = settingsTabs.includes("usage");
  const groups = useMemo(() => groupChannels(channels ?? []), [channels]);
  const layout = useMemo(() => layoutOf(channels ?? []), [channels]);
  const list = useRef<HTMLDivElement>(null);
  // Whoever can manage the server's channels drags them into order, Discord-style.
  useArrange({
    container: list,
    enabled: canCreate && !!synced,
    layout,
    onArrange: (next) => void run(reorderChannels(instanceKey, serverId, placements(next))).catch((err: FuwaError) => toast(err.message)),
  });
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [creating, setCreating] = useState<{ parentId: string } | null>(null);
  const [settings, setSettings] = useState<{ tab: string; target?: string } | null>(null);
  const [inviting, setInviting] = useState<string | null>(null);
  // The server menu's invite opens the channel you're in, as Discord's does, if you may invite to it.
  const inviteTo =
    params.channel && hasIn(access, params.channel, Permission.CREATE_INVITE) ? params.channel : has(access, Permission.CREATE_INVITE) ? "" : null;
  const leave = useAction(leaveServer);
  const developer = usePrefs((p) => p.developerMode);
  const [reading, setReading] = useState(false);
  const [welcoming, setWelcoming] = useState(false);
  // People waiting to be let in, for whoever can let them in.
  const reviews = !!server?.applications && has(access, Permission.KICK_MEMBERS);
  // Locked out until they sign in through the server's provider: the way back in, where channels would be.
  const ssoLocked = useSsoLocked(instanceKey, serverId);
  const { compact, setNavOpen } = useLayout();
  const waiting = useFuwa((s) => s.instances[instanceKey]?.applications[serverId]?.length ?? 0);
  useEffect(() => {
    if (reviews) run(listApplications(instanceKey, serverId)).catch(() => {});
  }, [reviews, instanceKey, serverId]);

  const edit = (id: string, focus?: "permissions") => setSettings({ tab: "channels", target: focus ? `${id}:${focus}` : id });

  if (!known) return null;
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            className="group flex h-14 shrink-0 items-center gap-2 border-b px-4 text-left transition hover:bg-muted/60 data-[state=open]:bg-muted/60"
          >
            <span className="min-w-0 flex-1">
              <span className="block truncate font-extrabold">
                <SwapText className="truncate align-bottom">{server?.name ?? "…"}</SwapText>
              </span>
              <span className="block truncate text-xs text-muted-foreground">
                {server ? (
                  <T
                    k="workspace.sidebar.membersOn"
                    values={{ count: <Count value={Number(server.memberCount)} />, place: nodeName ?? <Private text={instanceKey} /> }}
                    count={Number(server.memberCount)}
                  />
                ) : (
                  (nodeName ?? <Private text={instanceKey} />)
                )}
              </span>
            </span>
            <ChevronDownIcon className="size-4 transition-transform duration-300 group-data-[state=open]:rotate-180" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-60">
          {inviteTo !== null && (
            <DropdownMenuItem onSelect={() => setInviting(inviteTo)} className="font-bold text-primary focus:text-primary [&_svg]:text-primary">
              <UserPlusIcon /> {t("workspace.sidebar.invite")}
            </DropdownMenuItem>
          )}
          {inviteTo !== null && <DropdownMenuSeparator />}
          {settingsTabs.length > 0 && (
            <DropdownMenuItem onSelect={() => setSettings({ tab: settingsTabs[0]! })}>
              <SettingsIcon /> {t("workspace.sidebar.serverSettings")}
            </DropdownMenuItem>
          )}
          {reviews && (
            <DropdownMenuItem onSelect={() => setSettings({ tab: "applications" })}>
              <ClipboardListIcon /> {t("workspace.sidebar.applications")}
              {waiting > 0 && (
                <span className="ml-auto grid h-5 min-w-5 place-items-center rounded-full bg-destructive px-1.5 text-[0.65rem] font-extrabold text-white">
                  <Count value={waiting} max={99} />
                </span>
              )}
            </DropdownMenuItem>
          )}
          {usage && (
            <DropdownMenuItem onSelect={() => setSettings({ tab: "usage" })}>
              <ChartColumnIcon /> {t("workspace.sidebar.usage")}
            </DropdownMenuItem>
          )}
          {canCreate && (
            <DropdownMenuItem onSelect={() => setCreating({ parentId: "" })}>
              <PlusIcon /> {t("workspace.sidebar.createChannel")}
            </DropdownMenuItem>
          )}
          {(settingsTabs.length > 0 || canCreate) && <DropdownMenuSeparator />}
          {server?.hasRules && (
            <DropdownMenuItem onSelect={() => setReading(true)}>
              <ScrollTextIcon /> {t("workspace.sidebar.rules")}
            </DropdownMenuItem>
          )}
          {(server?.hasWelcomeScreen || server?.hasOnboarding) && (
            <DropdownMenuItem onSelect={() => setWelcoming(true)}>
              <PartyPopperIcon /> {server.hasOnboarding ? t("workspace.sidebar.channelsRoles") : t("workspace.sidebar.welcomeScreen")}
            </DropdownMenuItem>
          )}
          <ServerNotificationItems instanceKey={instanceKey} serverId={serverId} />
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => openSettings("server-profiles", serverId)}>
            <IdCardIcon /> {t("workspace.sidebar.editServerProfile")}
          </DropdownMenuItem>
          {developer && (
            <DropdownMenuItem onSelect={() => copy(t, serverId, t("common.copy.serverId"))}>
              <FingerprintIcon /> {t("workspace.sidebar.copyServerId")}
            </DropdownMenuItem>
          )}
          {!owner && <DropdownMenuSeparator />}
          {!owner && (
            <DropdownMenuItem
              variant="destructive"
              disabled={leave.pending}
              onSelect={async () => {
                if ((await leave.go(instanceKey, serverId)) !== undefined)
                  navigate({ to: "/$instance", params: { instance: instanceKey } });
              }}
            >
              <DoorOpenIcon /> {t("workspace.sidebar.leaveServer")}
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <div ref={list} className="scroll-thin relative flex-1 overflow-y-auto px-2 pt-3 pb-4">
        <AnimatePresence initial={false}>
          {reviews && waiting > 0 && (
            <motion.button
              type="button"
              onClick={() => setSettings({ tab: "applications" })}
              initial={{ opacity: 0, height: 0, marginBottom: 0 }}
              animate={{ opacity: 1, height: "auto", marginBottom: 12 }}
              exit={{ opacity: 0, height: 0, marginBottom: 0 }}
              transition={SPRING}
              className="group flex w-full items-center gap-2 overflow-hidden rounded-xl bg-primary/10 px-2.5 py-2 text-left text-sm font-bold text-primary transition-colors hover:bg-primary/15"
            >
              <span className="relative grid size-7 shrink-0 place-items-center rounded-lg bg-primary text-primary-foreground">
                <ClipboardListIcon className="size-4" />
                <motion.span aria-hidden animate={{ scale: [1, 1.6], opacity: [0.6, 0] }} transition={{ duration: 1.6, repeat: Infinity }} className="absolute inset-0 rounded-lg bg-primary" />
              </span>
              <span className="min-w-0 flex-1 truncate">
                <T k="workspace.sidebar.waiting" values={{ count: <Count value={waiting} /> }} count={waiting} />
              </span>
              <ChevronRightIcon className="size-4 transition-transform group-hover:translate-x-0.5" />
            </motion.button>
          )}
        </AnimatePresence>
        <AnimatePresence initial={false}>
          {server && ssoLocked && (
            <motion.button
              type="button"
              data-testid="sso-sidebar-locked"
              onClick={() => {
                navigate({ to: "/$instance/$server", params: { instance: instanceKey, server: serverId } });
                if (compact) setNavOpen(false);
              }}
              initial={{ opacity: 0, height: 0, marginBottom: 0 }}
              animate={{ opacity: 1, height: "auto", marginBottom: 12 }}
              exit={{ opacity: 0, height: 0, marginBottom: 0 }}
              transition={SPRING}
              className="group flex w-full items-center gap-2 overflow-hidden rounded-xl bg-primary/10 px-2.5 py-2 text-left text-sm font-bold text-primary transition-colors hover:bg-primary/15"
            >
              <span className="grid size-7 shrink-0 place-items-center rounded-lg bg-primary text-primary-foreground">
                <LockKeyholeIcon className="size-4 transition-transform group-hover:-rotate-12" />
              </span>
              <span className="min-w-0 flex-1 truncate">{t("workspace.sidebar.ssoSignIn", { provider: server.ssoName || t("workspace.sidebar.ssoYourOrganization") })}</span>
              <ChevronRightIcon className="size-4 transition-transform group-hover:translate-x-0.5" />
            </motion.button>
          )}
        </AnimatePresence>
        {!synced && !channels?.length ? (
          <div className="flex flex-col gap-2 px-2">
            {[70, 55, 80, 45].map((w, n) => (
              <div key={n} className="shimmer h-5 rounded-md" style={{ width: `${w}%` }} />
            ))}
          </div>
        ) : (
          <ul className="flex flex-col gap-0.5">
            <AnimatePresence mode="popLayout" initial={false}>
              {groups.flatMap((group) => {
                const id = group.category?.id ?? "";
                const closed = !!collapsed[id];
                const rows = closed
                  ? []
                  : group.channels.map((c, n) => (
                      <ChannelRow
                        key={c.id}
                        index={n}
                        parent={id}
                        instanceKey={instanceKey}
                        channel={c}
                        active={params.channel === c.id}
                        canConnect={hasIn(access, c.id, Permission.CONNECT)}
                        onEdit={
                          hasIn(access, c.id, Permission.MANAGE_CHANNELS) || hasIn(access, c.id, Permission.MANAGE_ROLES)
                            ? () => setSettings({ tab: "channels", target: c.id })
                            : undefined
                        }
                        onInvite={c.type !== ChannelType.VOICE && c.type !== ChannelType.SECURE && hasIn(access, c.id, Permission.CREATE_INVITE) ? () => setInviting(c.id) : undefined}
                        onMenuEdit={edit}
                      />
                    ));
                if (!group.category) return rows;
                return [
                  <CategoryRow
                    key={id}
                    category={group.category}
                    closed={closed}
                    count={group.channels.length}
                    onToggle={() => setCollapsed((c) => ({ ...c, [id]: !closed }))}
                    onAdd={hasIn(access, id, Permission.MANAGE_CHANNELS) ? () => setCreating({ parentId: id }) : undefined}
                    instanceKey={instanceKey}
                    onMenuEdit={edit}
                  />,
                  ...rows,
                ];
              })}
            </AnimatePresence>
          </ul>
        )}
      </div>
      <CallPanel />
      <UserPanel instanceKey={instanceKey} />

      <InviteDialog
        open={inviting !== null}
        onOpenChange={(open) => !open && setInviting(null)}
        instanceKey={instanceKey}
        serverId={serverId}
        channelId={inviting ?? ""}
      />
      <CreateChannelDialog
        open={!!creating}
        onOpenChange={(open) => !open && setCreating(null)}
        instanceKey={instanceKey}
        serverId={serverId}
        parentId={creating?.parentId}
      />
      {server && <WelcomeGate instanceKey={instanceKey} server={server} open={welcoming} onOpenChange={setWelcoming} />}
      {server && <RulesDialog open={reading} onOpenChange={setReading} instanceKey={instanceKey} server={server} agree={access.pending} />}
      {server && (
        <ServerSettingsDialog
          open={!!settings}
          onOpenChange={(open) => !open && setSettings(null)}
          instanceKey={instanceKey}
          server={server}
          tab={settings?.tab ?? "overview"}
          target={settings?.target}
        />
      )}
    </>
  );
}

/** A category's header: folds its channels away, and (with Manage Channels) adds one or drags the whole category. */
function CategoryRow({
  instanceKey,
  category,
  closed,
  count,
  onToggle,
  onAdd,
  onMenuEdit,
  ref,
}: {
  instanceKey: string;
  category: Channel;
  closed: boolean;
  count: number;
  onToggle: () => void;
  onAdd?: () => void;
  /** Opens its settings from its right-click menu. */
  onMenuEdit: ChannelMenuActions["edit"];
  ref?: Ref<HTMLLIElement>;
}) {
  const { t } = useI18n();
  const menu = useContextMenu("category", () =>
    categoryMenu(
      { instanceKey, serverId: category.serverId, category },
      { edit: onMenuEdit, toggle: onToggle, collapsed: closed, create: onAdd && (() => onAdd()) },
    ),
  );
  return (
    <motion.li
      ref={ref}
      data-arrange="category"
      data-id={category.id}
      data-collapsed={closed || undefined}
      layout="position"
      initial={{ opacity: 0, y: -6 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, transition: { duration: 0.15 } }}
      transition={SPRING}
      className="group relative mt-4 flex items-center rounded-lg pr-1 transition-colors"
    >
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={!closed}
        {...menu}
        className="flex min-w-0 flex-1 items-center gap-1 rounded-md px-1 py-1 text-xs font-bold tracking-wide text-muted-foreground uppercase transition group-data-[drop-into]:text-primary hover:text-foreground data-[menu-open]:bg-muted/70 data-[menu-open]:text-foreground"
      >
        <ChevronDownIcon className={cn("size-3 shrink-0 transition-transform duration-200", closed && "-rotate-90")} />
        <span className="truncate">{category.name}</span>
        <AnimatePresence initial={false}>
          {closed && count > 0 && (
            <motion.span
              initial={{ opacity: 0, scale: 0.6 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.6 }}
              transition={SPRING}
              className="ml-0.5 rounded-full bg-muted px-1.5 text-[0.62rem] tabular-nums"
            >
              {count}
            </motion.span>
          )}
        </AnimatePresence>
      </button>
      {onAdd && (
        <button
          type="button"
          data-arrange-skip
          aria-label={t("workspace.sidebar.createIn", { category: category.name })}
          onClick={onAdd}
          className="grid size-5 place-items-center rounded text-muted-foreground opacity-0 transition group-hover:opacity-100 hover:rotate-90 hover:text-foreground"
        >
          <PlusIcon className="size-3.5" />
        </button>
      )}
    </motion.li>
  );
}

function ChannelRow({
  instanceKey,
  channel,
  active,
  index,
  parent,
  ref,
  onEdit,
  onInvite,
  onMenuEdit,
  canConnect,
}: {
  instanceKey: string;
  /** A voice channel you may join. */
  canConnect?: boolean;
  channel: Channel;
  active: boolean;
  index: number;
  /** The category it shows under, or "" for none. */
  parent: string;
  ref?: Ref<HTMLLIElement>;
  /** With Manage Channels or Manage Roles there: opens the channel's settings. */
  onEdit?: () => void;
  /** With Create Invite there: invites people straight into it. */
  onInvite?: () => void;
  /** Opens its settings (or its permissions) from its right-click menu. */
  onMenuEdit: ChannelMenuActions["edit"];
}) {
  const { t } = useI18n();
  const muted = useMuted(instanceKey, channel.serverId, channel.id);
  const unread = useFuwa((s) => (muted ? 0 : (s.instances[instanceKey]?.unread[channel.id] ?? 0)));
  const { compact, setNavOpen } = useLayout();
  const Icon = CHANNEL_ICON[channel.type] ?? HashIcon;
  const dot = unread > 0 && !active;
  const locked = isPrivate(channel, channel.serverId);
  const voice = channel.type === ChannelType.VOICE;
  const menu = useContextMenu("channel", () => channelMenu({ instanceKey, serverId: channel.serverId, channel }, { invite: onInvite, edit: onMenuEdit }));
  return (
    <motion.li
      ref={ref}
      data-arrange="channel"
      data-id={channel.id}
      data-parent={parent}
      layout="position"
      initial={{ opacity: 0, x: -10 }}
      animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: Math.min(index, 12) * 0.025 } }}
      exit={{ opacity: 0, x: -10, transition: { duration: 0.15 } }}
      transition={SPRING}
      className="relative"
    >
      {active && (
        <motion.span
          layoutId={`channel-active-${instanceKey}`}
          className="absolute inset-0 rounded-lg bg-primary/15"
          transition={{ type: "spring", stiffness: 500, damping: 40 }}
        />
      )}
      <Link
        to="/$instance/$server/$channel"
        params={{ instance: instanceKey, server: channel.serverId, channel: channel.id }}
        onClick={() => {
          if (compact) setNavOpen(false);
          // A voice channel joins as it opens, as Discord's do, when you may connect there.
          if (voice && canConnect) void joinCall({ kind: "voice", instance: instanceKey, serverId: channel.serverId, channelId: channel.id });
        }}
        {...menu}
        className={cn(
          "row-y group relative flex items-center gap-1.5 rounded-lg px-2 text-[0.94rem] transition-colors data-[menu-open]:bg-muted/70 data-[menu-open]:text-foreground",
          active ? "font-bold text-primary" : unread ? "font-bold text-foreground" : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
          muted && !active && "opacity-55 hover:opacity-100",
        )}
      >
        <motion.span
          aria-hidden
          initial={false}
          animate={{ height: dot ? 8 : 0, opacity: dot ? 1 : 0 }}
          transition={SPRING}
          className="absolute top-1/2 -left-2 w-1 -translate-y-1/2 rounded-r-full bg-foreground"
        />
        <span className="relative shrink-0" title={locked ? t("workspace.sidebar.privateChannel") : undefined}>
          <Icon
            className={cn(
              "size-[18px] opacity-70 transition duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:-rotate-12 group-hover:scale-110 group-hover:opacity-100",
              active && "opacity-100",
            )}
          />
          <AnimatePresence initial={false}>
            {locked && (
              <motion.span
                initial={{ scale: 0 }}
                animate={{ scale: 1 }}
                exit={{ scale: 0 }}
                transition={{ type: "spring", stiffness: 600, damping: 18 }}
                className="absolute -right-1 -bottom-0.5 grid size-2.5 place-items-center rounded-full bg-background"
              >
                <LockIcon className="size-2" strokeWidth={3} />
              </motion.span>
            )}
          </AnimatePresence>
        </span>
        <span className="truncate">{channel.name}</span>
        <SharedBadge channel={channel} />
        {onInvite && (
          <span
            role="button"
            tabIndex={-1}
            aria-label={t("workspace.sidebar.inviteTo", { channel: channel.name })}
            title={t("workspace.sidebar.invite")}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              onInvite();
            }}
            className={cn(
              "ml-auto size-5 shrink-0 place-items-center rounded text-muted-foreground transition group-hover:grid hover:scale-110 hover:text-foreground",
              active ? "grid" : "hidden",
            )}
          >
            <UserPlusIcon className="size-3.5" />
          </span>
        )}
        {onEdit && (
          <span
            role="button"
            tabIndex={-1}
            aria-label={t("workspace.sidebar.editChannelNamed", { channel: channel.name })}
            title={t("workspace.sidebar.editChannel")}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              onEdit();
            }}
            className={cn(
              !onInvite && "ml-auto",
              "size-5 shrink-0 place-items-center rounded text-muted-foreground transition group-hover:grid hover:rotate-45 hover:text-foreground",
              // Like Discord: always there on the channel you're in, on hover elsewhere.
              active ? "grid" : "hidden",
            )}
          >
            <SettingsIcon className="size-3.5" />
          </span>
        )}
        <AnimatePresence>
          {muted && (
            <motion.span
              key="muted"
              initial={{ scale: 0, rotate: -30 }}
              animate={{ scale: 1, rotate: 0 }}
              exit={{ scale: 0, rotate: 30 }}
              transition={{ type: "spring", stiffness: 600, damping: 18 }}
              className="ml-auto shrink-0"
              aria-label={t("workspace.sidebar.muted")}
            >
              <BellOffIcon className="size-3.5" />
            </motion.span>
          )}
        </AnimatePresence>
        <AnimatePresence>
          {unread > 0 && !active && (
            <motion.span
              initial={{ scale: 0 }}
              animate={{ scale: 1 }}
              exit={{ scale: 0 }}
              transition={{ type: "spring", stiffness: 600, damping: 20 }}
              className="ml-auto grid h-5 min-w-5 place-items-center rounded-full bg-destructive px-1.5 text-[0.7rem] font-extrabold text-white"
            >
              <Count value={unread} max={99} />
            </motion.span>
          )}
        </AnimatePresence>
      </Link>
      {voice && <VoiceUsers instanceKey={instanceKey} serverId={channel.serverId} channelId={channel.id} />}
    </motion.li>
  );
}
