import { Link, useNavigate, useParams } from "@tanstack/react-router";
import {
  ChartColumnIcon,
  ChevronDownIcon,
  DoorOpenIcon,
  HashIcon,
  MegaphoneIcon,
  PlusIcon,
  SettingsIcon,
  Volume2Icon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useMemo, useState } from "react";
import { ChannelType, MemberRole, type Channel } from "@/gen/fuwa/v1/types_pb";
import { leaveServer } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { CreateChannelDialog } from "@/components/dialogs/CreateChannelDialog";
import { ServerSettingsDialog } from "@/components/dialogs/ServerSettingsDialog";
import { useLayout } from "@/components/Shell";
import { UserPanel } from "@/components/UserPanel";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

export const CHANNEL_ICON: Partial<Record<ChannelType, typeof HashIcon>> = {
  [ChannelType.ANNOUNCEMENT]: MegaphoneIcon,
  [ChannelType.VOICE]: Volume2Icon,
};

/** The role you have in a server, from its member list. */
export function useMyRole(instanceKey: string, serverId: string): MemberRole {
  return useFuwa((s) => {
    const i = s.instances[instanceKey];
    const me = i?.me?.id;
    return i?.members[serverId]?.find((m) => m.user?.id === me)?.role ?? MemberRole.MEMBER;
  });
}

type Group = { category: Channel | null; channels: Channel[] };

function groupChannels(channels: Channel[]): Group[] {
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

export function ChannelSidebar({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const inst = useInstance(instanceKey);
  const params = useParams({ strict: false }) as { channel?: string };
  const navigate = useNavigate();
  const server = inst?.servers.find((s) => s.id === serverId);
  const channels = inst?.channels[serverId];
  const synced = inst?.synced[serverId];
  const role = useMyRole(instanceKey, serverId);
  const manager = role >= MemberRole.ADMIN || !!inst?.admin;
  const owner = role === MemberRole.OWNER;
  const groups = useMemo(() => groupChannels(channels ?? []), [channels]);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [creating, setCreating] = useState<{ parentId: string } | null>(null);
  const [settings, setSettings] = useState<string | null>(null);
  const leave = useAction(leaveServer);

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
              <span className="block truncate font-extrabold">{server?.name ?? "…"}</span>
              <span className="block truncate text-xs text-muted-foreground">
                {server ? `${Number(server.memberCount)} ${server.memberCount === 1n ? "member" : "members"} · ` : ""}
                {inst.node?.name ?? instanceKey}
              </span>
            </span>
            <ChevronDownIcon className="size-4 transition-transform duration-300 group-data-[state=open]:rotate-180" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-60">
          {manager && (
            <DropdownMenuItem onSelect={() => setSettings("overview")}>
              <SettingsIcon /> Server settings
            </DropdownMenuItem>
          )}
          {manager && (
            <DropdownMenuItem onSelect={() => setSettings("usage")}>
              <ChartColumnIcon /> Usage
            </DropdownMenuItem>
          )}
          {manager && (
            <DropdownMenuItem onSelect={() => setCreating({ parentId: "" })}>
              <PlusIcon /> Create channel
            </DropdownMenuItem>
          )}
          {manager && !owner && <DropdownMenuSeparator />}
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
                    {manager && (
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
                      {group.channels.map((c) => (
                        <ChannelRow key={c.id} instanceKey={instanceKey} channel={c} active={params.channel === c.id} />
                      ))}
                    </motion.ul>
                  )}
                </AnimatePresence>
              </div>
            );
          })
        )}
      </div>
      <UserPanel instanceKey={instanceKey} />

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
          isOwner={owner || !!inst.admin}
          instanceAdmin={!!inst.admin}
          tab={settings ?? "overview"}
        />
      )}
    </>
  );
}

function ChannelRow({ instanceKey, channel, active }: { instanceKey: string; channel: Channel; active: boolean }) {
  const unread = useFuwa((s) => s.instances[instanceKey]?.unread[channel.id] ?? 0);
  const { compact, setNavOpen } = useLayout();
  const Icon = CHANNEL_ICON[channel.type] ?? HashIcon;
  return (
    <li className="relative">
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
          "group relative flex items-center gap-1.5 rounded-lg px-2 py-1.5 text-[0.94rem] transition-colors",
          active ? "font-bold text-primary" : unread ? "font-bold text-foreground" : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
        )}
      >
        {unread > 0 && !active && <span className="absolute top-1/2 -left-2 h-2 w-1 -translate-y-1/2 rounded-r-full bg-foreground" />}
        <Icon className={cn("size-[18px] shrink-0 opacity-70 transition group-hover:opacity-100", active && "opacity-100")} />
        <span className="truncate">{channel.name}</span>
        <AnimatePresence>
          {unread > 0 && !active && (
            <motion.span
              initial={{ scale: 0 }}
              animate={{ scale: 1 }}
              exit={{ scale: 0 }}
              transition={{ type: "spring", stiffness: 600, damping: 20 }}
              className="ml-auto grid h-5 min-w-5 place-items-center rounded-full bg-destructive px-1.5 text-[0.7rem] font-extrabold text-white"
            >
              {unread > 99 ? "99+" : unread}
            </motion.span>
          )}
        </AnimatePresence>
      </Link>
    </li>
  );
}
