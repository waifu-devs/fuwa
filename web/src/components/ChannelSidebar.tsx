import { Link, useNavigate, useParams } from "@tanstack/react-router";
import {
  BellIcon,
  BellOffIcon,
  BellRingIcon,
  ChartColumnIcon,
  ChevronDownIcon,
  DoorOpenIcon,
  FingerprintIcon,
  HashIcon,
  IdCardIcon,
  LockIcon,
  MegaphoneIcon,
  PlusIcon,
  SettingsIcon,
  UserPlusIcon,
  Volume2Icon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useMemo, useState, type Ref } from "react";
import { ChannelType, Permission, type Channel } from "@/gen/fuwa/v1/types_pb";
import { leaveServer, run, updateNotifications } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess, useAction, useInstance } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { CreateChannelDialog } from "@/components/dialogs/CreateChannelDialog";
import { InviteDialog } from "@/components/dialogs/InviteDialog";
import { ServerSettingsDialog, useServerSettingsTabs } from "@/components/dialogs/ServerSettingsDialog";
import { useLayout } from "@/components/Shell";
import { Count, SPRING, SwapText } from "@/components/motion";
import { Private } from "@/components/Private";
import { UserPanel } from "@/components/UserPanel";
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
import { isMuted, MUTE_FOR, mutedLabel, useMuted, useNotificationSettings, useNow } from "@/lib/notifications";
import { has, hasIn, isPrivate } from "@/lib/permissions";
import { usePrefs } from "@/lib/prefs";
import { copy, openSettings, toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

export const CHANNEL_ICON: Partial<Record<ChannelType, typeof HashIcon>> = {
  [ChannelType.ANNOUNCEMENT]: MegaphoneIcon,
  [ChannelType.VOICE]: Volume2Icon,
};

type Group = { category: Channel | null; channels: Channel[] };

/** Channels you can open, in the order the sidebar lists them. */
export const openableChannels = (channels: Channel[]) =>
  groupChannels(channels)
    .flatMap((g) => g.channels)
    .filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT);

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
  const now = useNow();
  const settings = useNotificationSettings(instanceKey, serverId);
  const mute = (mutedUntil: Date | null | false) =>
    run(updateNotifications(instanceKey, serverId, "", { mutedUntil })).catch((err: FuwaError) => toast(err.message));
  return (
    <>
      {isMuted(settings, now) ? (
        <DropdownMenuItem onSelect={() => void mute(false)}>
          <BellIcon /> Unmute server
          <span className="ml-auto truncate pl-2 text-xs text-muted-foreground">{mutedLabel(settings, now).replace(/^Muted /, "")}</span>
        </DropdownMenuItem>
      ) : (
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <BellOffIcon /> Mute server
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-52">
            {MUTE_FOR.map((m) => (
              <DropdownMenuItem key={m.label} onSelect={() => void mute(m.ms === null ? null : new Date(Date.now() + m.ms))}>
                {m.label}
              </DropdownMenuItem>
            ))}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
      )}
      <DropdownMenuItem onSelect={() => openSettings("server-notifications", serverId)}>
        <BellRingIcon /> Notification settings
      </DropdownMenuItem>
    </>
  );
}

export function ChannelSidebar({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const inst = useInstance(instanceKey);
  const params = useParams({ strict: false }) as { channel?: string };
  const navigate = useNavigate();
  const server = inst?.servers.find((s) => s.id === serverId);
  const channels = inst?.channels[serverId];
  const synced = inst?.synced[serverId];
  const access = useAccess(instanceKey, serverId);
  const owner = access.owner;
  const settingsTabs = useServerSettingsTabs(instanceKey, serverId);
  const canCreate = has(access, Permission.MANAGE_CHANNELS);
  const usage = settingsTabs.includes("usage");
  const groups = useMemo(() => groupChannels(channels ?? []), [channels]);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [creating, setCreating] = useState<{ parentId: string } | null>(null);
  const [settings, setSettings] = useState<{ tab: string; target?: string } | null>(null);
  const [inviting, setInviting] = useState<string | null>(null);
  // The server menu's invite opens the channel you're in, as Discord's does, if you may invite to it.
  const inviteTo =
    params.channel && hasIn(access, params.channel, Permission.CREATE_INVITE) ? params.channel : has(access, Permission.CREATE_INVITE) ? "" : null;
  const leave = useAction(leaveServer);
  const developer = usePrefs((p) => p.developerMode);

  if (!inst) return null;
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
                {server && (
                  <>
                    <Count value={Number(server.memberCount)} /> {server.memberCount === 1n ? "member" : "members"} ·{" "}
                  </>
                )}
                {inst.node?.name ?? <Private text={instanceKey} />}
              </span>
            </span>
            <ChevronDownIcon className="size-4 transition-transform duration-300 group-data-[state=open]:rotate-180" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-60">
          {inviteTo !== null && (
            <DropdownMenuItem onSelect={() => setInviting(inviteTo)} className="font-bold text-primary focus:text-primary [&_svg]:text-primary">
              <UserPlusIcon /> Invite people
            </DropdownMenuItem>
          )}
          {inviteTo !== null && <DropdownMenuSeparator />}
          {settingsTabs.length > 0 && (
            <DropdownMenuItem onSelect={() => setSettings({ tab: settingsTabs[0]! })}>
              <SettingsIcon /> Server settings
            </DropdownMenuItem>
          )}
          {usage && (
            <DropdownMenuItem onSelect={() => setSettings({ tab: "usage" })}>
              <ChartColumnIcon /> Usage
            </DropdownMenuItem>
          )}
          {canCreate && (
            <DropdownMenuItem onSelect={() => setCreating({ parentId: "" })}>
              <PlusIcon /> Create channel
            </DropdownMenuItem>
          )}
          {(settingsTabs.length > 0 || canCreate) && <DropdownMenuSeparator />}
          <ServerNotificationItems instanceKey={instanceKey} serverId={serverId} />
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => openSettings("server-profiles", serverId)}>
            <IdCardIcon /> Edit server profile
          </DropdownMenuItem>
          {developer && (
            <DropdownMenuItem onSelect={() => copy(serverId, "server ID")}>
              <FingerprintIcon /> Copy server ID
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
              <DoorOpenIcon /> Leave server
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <div className="scroll-thin flex-1 overflow-y-auto px-2 pt-3 pb-4">
        {!synced && !channels?.length ? (
          <div className="flex flex-col gap-2 px-2">
            {[70, 55, 80, 45].map((w, n) => (
              <div key={n} className="shimmer h-5 rounded-md" style={{ width: `${w}%` }} />
            ))}
          </div>
        ) : (
          groups.map((group) => {
            const id = group.category?.id ?? "";
            const closed = !!collapsed[id];
            return (
              <div key={id || "loose"} className={cn(group.category && "mt-4")}>
                {group.category && (
                  <div className="group flex items-center pr-1">
                    <button
                      type="button"
                      onClick={() => setCollapsed((c) => ({ ...c, [id]: !closed }))}
                      className="flex flex-1 items-center gap-1 px-1 py-1 text-xs font-bold tracking-wide text-muted-foreground uppercase transition hover:text-foreground"
                    >
                      <ChevronDownIcon className={cn("size-3 transition-transform duration-200", closed && "-rotate-90")} />
                      <span className="truncate">{group.category.name}</span>
                    </button>
                    {hasIn(access, id, Permission.MANAGE_CHANNELS) && (
                      <button
                        type="button"
                        aria-label={`Create a channel in ${group.category.name}`}
                        onClick={() => setCreating({ parentId: id })}
                        className="grid size-5 place-items-center rounded text-muted-foreground opacity-0 transition group-hover:opacity-100 hover:text-foreground"
                      >
                        <PlusIcon className="size-3.5" />
                      </button>
                    )}
                  </div>
                )}
                <AnimatePresence initial={false}>
                  {!closed && (
                    <motion.ul
                      initial={{ height: 0, opacity: 0 }}
                      animate={{ height: "auto", opacity: 1 }}
                      exit={{ height: 0, opacity: 0 }}
                      transition={{ duration: 0.22, ease: [0.22, 1, 0.36, 1] }}
                      className="flex flex-col gap-0.5 overflow-hidden"
                    >
                      <AnimatePresence mode="popLayout">
                        {group.channels.map((c, n) => (
                          <ChannelRow
                            key={c.id}
                            index={n}
                            instanceKey={instanceKey}
                            channel={c}
                            active={params.channel === c.id}
                            onEdit={
                              hasIn(access, c.id, Permission.MANAGE_CHANNELS) || hasIn(access, c.id, Permission.MANAGE_ROLES)
                                ? () => setSettings({ tab: "channels", target: c.id })
                                : undefined
                            }
                            onInvite={
                              c.type !== ChannelType.VOICE && hasIn(access, c.id, Permission.CREATE_INVITE) ? () => setInviting(c.id) : undefined
                            }
                          />
                        ))}
                      </AnimatePresence>
                    </motion.ul>
                  )}
                </AnimatePresence>
              </div>
            );
          })
        )}
      </div>
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

function ChannelRow({
  instanceKey,
  channel,
  active,
  index,
  ref,
  onEdit,
  onInvite,
}: {
  instanceKey: string;
  channel: Channel;
  active: boolean;
  index: number;
  ref?: Ref<HTMLLIElement>;
  /** With Manage Channels or Manage Roles there: opens the channel's settings. */
  onEdit?: () => void;
  /** With Create Invite there: invites people straight into it. */
  onInvite?: () => void;
}) {
  const muted = useMuted(instanceKey, channel.serverId, channel.id);
  const unread = useFuwa((s) => (muted ? 0 : (s.instances[instanceKey]?.unread[channel.id] ?? 0)));
  const { compact, setNavOpen } = useLayout();
  const Icon = CHANNEL_ICON[channel.type] ?? HashIcon;
  const dot = unread > 0 && !active;
  const locked = isPrivate(channel, channel.serverId);
  return (
    <motion.li
      ref={ref}
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
        onClick={() => compact && setNavOpen(false)}
        className={cn(
          "row-y group relative flex items-center gap-1.5 rounded-lg px-2 text-[0.94rem] transition-colors",
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
        <span className="relative shrink-0" title={locked ? "Private channel" : undefined}>
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
        {onInvite && (
          <span
            role="button"
            tabIndex={-1}
            aria-label={`Invite people to #${channel.name}`}
            title="Invite people"
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
            aria-label={`Edit #${channel.name}`}
            title="Edit channel"
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
              aria-label="Muted"
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
    </motion.li>
  );
}
