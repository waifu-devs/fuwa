import { BellIcon, BellOffIcon, ChevronDownIcon, HashIcon, PlusIcon, ServerIcon as ServerGlyph, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { NotificationLevel, type Channel, type NotificationSettings, type Server } from "@/gen/fuwa/v1/types_pb";
import { run, updateNotifications, type NotificationPatch } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { notificationKey, useFuwa } from "@/fuwa/store";
import { CHANNEL_ICON, openableChannels } from "@/components/ChannelSidebar";
import { ServerIcon } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { Segmented } from "@/components/settings/account/common";
import { Toggle } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { isMuted, LEVELS, MUTE_FOR, mutedLabel, useNow } from "@/lib/notifications";
import { toast, useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

const LEVEL_OPTIONS = [{ value: NotificationLevel.UNSPECIFIED, label: "Default" }, ...LEVELS.map((l) => ({ value: l.value, label: l.short }))];

const levelLabel = (level: NotificationLevel | undefined) => LEVELS.find((l) => l.value === level)?.label ?? "Default";

/** Saves a change and says so if it didn't go through. */
function change(key: string, serverId: string, channelId: string, patch: NotificationPatch) {
  run(updateNotifications(key, serverId, channelId, patch)).catch((err: FuwaError) => toast(err.message));
}

/**
 * How each server and channel notifies you. Kept on the instance, so they
 * follow you to every device; anything left on Default goes by this
 * device's own Notifications settings.
 */
export function ServerNotifications({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const target = useUi((u) => u.settingsTarget);
  const servers = inst?.servers ?? [];
  const [open, setOpen] = useState<string | null>(() => (target && servers.some((s) => s.id === target) ? target : null));

  useEffect(() => {
    if (target && servers.some((s) => s.id === target)) setOpen(target);
  }, [target, servers]);

  if (!servers.length) {
    return (
      <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col items-center gap-2 rounded-3xl border border-dashed px-6 py-12 text-center">
        <ServerGlyph className="size-8 text-muted-foreground" />
        <p className="font-extrabold">No servers yet</p>
        <p className="max-w-sm text-sm text-muted-foreground">Once you join a server you can choose how it notifies you here.</p>
      </motion.div>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">These follow your account to every device. Anything on Default goes by this device's Notifications settings.</p>
      {servers.map((server, n) => (
        <ServerCard
          key={server.id}
          instanceKey={instanceKey}
          server={server}
          open={open === server.id}
          onToggle={() => setOpen((o) => (o === server.id ? null : server.id))}
          highlight={target === server.id}
          delay={n * 0.04}
        />
      ))}
    </div>
  );
}

function ServerCard({
  instanceKey,
  server,
  open,
  onToggle,
  highlight,
  delay,
}: {
  instanceKey: string;
  server: Server;
  open: boolean;
  onToggle: () => void;
  highlight: boolean;
  delay: number;
}) {
  const now = useNow();
  const settings = useFuwa((s) => s.instances[instanceKey]?.notifications[notificationKey(server.id)]);
  const muted = isMuted(settings, now);
  const ref = useRef<HTMLDivElement>(null);

  // Opened for this server from its menu: bring it into view and make it glow.
  useEffect(() => {
    if (!highlight || !ref.current) return;
    const el = ref.current;
    const t = setTimeout(() => {
      el.scrollIntoView({ behavior: "smooth", block: "start" });
      el.classList.add("found");
    }, 250);
    const off = setTimeout(() => el.classList.remove("found"), 2200);
    return () => {
      clearTimeout(t);
      clearTimeout(off);
    };
  }, [highlight]);

  return (
    <motion.div
      ref={ref}
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ ...SPRING, delay }}
      className={cn("scroll-mt-4 overflow-hidden rounded-2xl border bg-card transition-colors", open && "border-primary/40")}
    >
      <button type="button" onClick={onToggle} aria-expanded={open} className="flex w-full items-center gap-3 p-3 text-left transition hover:bg-muted/50">
        <ServerIcon server={server} className="size-10 rounded-xl text-sm" />
        <span className="min-w-0 flex-1">
          <span className="block truncate font-bold">{server.name}</span>
          <span className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
            <AnimatePresence mode="popLayout" initial={false}>
              {muted ? (
                <motion.span key="muted" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} className="flex min-w-0 items-center gap-1 font-bold text-amber-500">
                  <BellOffIcon className="size-3.5 shrink-0" />
                  <span className="truncate">{mutedLabel(settings, now)}</span>
                </motion.span>
              ) : (
                <motion.span key={settings?.level ?? 0} initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} className="flex items-center gap-1">
                  <BellIcon className="size-3.5" />
                  {levelLabel(settings?.level)}
                </motion.span>
              )}
            </AnimatePresence>
          </span>
        </span>
        <ChevronDownIcon className={cn("size-4 shrink-0 text-muted-foreground transition-transform duration-300", open && "rotate-180")} />
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div initial={{ height: 0, opacity: 0 }} animate={{ height: "auto", opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={{ ...SPRING, opacity: { duration: 0.2 } }} className="overflow-hidden">
            <ServerBody instanceKey={instanceKey} server={server} settings={settings} muted={muted} now={now} />
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  );
}

function ServerBody({ instanceKey, server, settings, muted, now }: { instanceKey: string; server: Server; settings: NotificationSettings | undefined; muted: boolean; now: number }) {
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[server.id]);
  const all = useFuwa((s) => s.instances[instanceKey]?.notifications);
  const [added, setAdded] = useState<string[]>([]);
  const openable = openableChannels(channels ?? []);
  const withSettings = openable.filter((c) => all?.[notificationKey(server.id, c.id)] || added.includes(c.id));
  const rest = openable.filter((c) => !withSettings.includes(c));

  return (
    <div className="flex flex-col gap-5 border-t p-4">
      <MuteControl muted={muted} label={mutedLabel(settings, now)} what="server" onMute={(mutedUntil) => change(instanceKey, server.id, "", { mutedUntil })} />
      <div className="flex flex-col gap-2">
        <p className="text-sm font-bold">Notify me about</p>
        <Segmented
          label="Notify me about"
          value={settings?.level ?? NotificationLevel.UNSPECIFIED}
          onChange={(level) => change(instanceKey, server.id, "", { level })}
          options={LEVEL_OPTIONS}
          className="w-full max-w-md"
        />
      </div>
      <Toggle
        label="Suppress @everyone and @here"
        hint="They won't count as mentions of you in this server."
        checked={!!settings?.suppressEveryone}
        onChange={(suppressEveryone) => change(instanceKey, server.id, "", { suppressEveryone })}
      />
      <div className="flex flex-col gap-2">
        <div className="flex items-center justify-between gap-2">
          <p className="text-sm font-bold">Channels</p>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button type="button" variant="outline" size="sm" className="rounded-xl" disabled={!rest.length}>
                <PlusIcon className="size-4" /> Add a channel
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="max-h-72 w-56 overflow-y-auto">
              <DropdownMenuLabel>Set a channel apart</DropdownMenuLabel>
              {rest.map((c) => {
                const Icon = CHANNEL_ICON[c.type] ?? HashIcon;
                return (
                  <DropdownMenuItem key={c.id} onSelect={() => setAdded((a) => [...a, c.id])}>
                    <Icon /> {c.name}
                  </DropdownMenuItem>
                );
              })}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        <motion.ul layout className="flex flex-col gap-2">
          <AnimatePresence initial={false} mode="popLayout">
            {withSettings.map((c) => (
              <motion.li key={c.id} layout initial={{ opacity: 0, y: -8, scale: 0.97 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, x: 40 }} transition={SPRING}>
                <ChannelRow
                  instanceKey={instanceKey}
                  serverId={server.id}
                  channel={c}
                  now={now}
                  // Kept on screen even when it goes back to all defaults, until removed.
                  onChange={(patch) => {
                    setAdded((a) => (a.includes(c.id) ? a : [...a, c.id]));
                    change(instanceKey, server.id, c.id, patch);
                  }}
                  onRemove={() => {
                    setAdded((a) => a.filter((id) => id !== c.id));
                    if (all?.[notificationKey(server.id, c.id)]) change(instanceKey, server.id, c.id, { level: NotificationLevel.UNSPECIFIED, mutedUntil: false });
                  }}
                />
              </motion.li>
            ))}
            {withSettings.length === 0 && (
              <motion.li key="none" layout initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="rounded-xl border border-dashed px-3 py-2.5 text-sm text-muted-foreground">
                Every channel follows the server. Add one to set it apart.
              </motion.li>
            )}
          </AnimatePresence>
        </motion.ul>
      </div>
    </div>
  );
}

function ChannelRow({
  instanceKey,
  serverId,
  channel,
  now,
  onChange,
  onRemove,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  now: number;
  onChange: (patch: NotificationPatch) => void;
  onRemove: () => void;
}) {
  const settings = useFuwa((s) => s.instances[instanceKey]?.notifications[notificationKey(serverId, channel.id)]);
  const muted = isMuted(settings, now);
  const Icon = CHANNEL_ICON[channel.type] ?? HashIcon;
  return (
    <div className="flex flex-col gap-3 rounded-xl border bg-background/50 p-3">
      <div className="flex items-center gap-2">
        <Icon className="size-4 shrink-0 text-muted-foreground" />
        <span className="min-w-0 flex-1 truncate text-sm font-bold">{channel.name}</span>
        <button
          type="button"
          onClick={onRemove}
          aria-label={`Stop setting ${channel.name} apart`}
          title="Follow the server again"
          className="grid size-7 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90"
        >
          <XIcon className="size-4" />
        </button>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Segmented
          label={`Notify me about #${channel.name}`}
          value={settings?.level ?? NotificationLevel.UNSPECIFIED}
          onChange={(level) => onChange({ level })}
          options={LEVEL_OPTIONS}
          className="w-full text-xs sm:w-auto sm:min-w-0 sm:flex-1 [&_button]:px-2"
        />
        <MuteControl compact muted={muted} label={mutedLabel(settings, now)} what="channel" onMute={(mutedUntil) => onChange({ mutedUntil })} />
      </div>
    </div>
  );
}

/** Mute for a while (picked from a menu), or unmute. */
function MuteControl({
  muted,
  label,
  what,
  onMute,
  compact = false,
}: {
  muted: boolean;
  label: string;
  what: "server" | "channel";
  onMute: (until: Date | null | false) => void;
  compact?: boolean;
}) {
  return (
    <div className={cn("flex items-center gap-3", !compact && "justify-between")}>
      {!compact && (
        <span className="min-w-0">
          <span className="block text-sm font-bold">Mute {what}</span>
          <span className="block text-xs text-muted-foreground">No sounds, notifications or unread badges{what === "server" ? " from any of its channels" : ""}.</span>
        </span>
      )}
      <AnimatePresence mode="popLayout" initial={false}>
        {muted ? (
          <motion.div key="muted" initial={{ opacity: 0, scale: 0.9 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.9 }} transition={SPRING}>
            <Button type="button" variant="outline" size="sm" onClick={() => onMute(false)} className="group rounded-xl border-amber-500/40 text-amber-600 hover:bg-amber-500/10 dark:text-amber-400" title="Unmute">
              <BellOffIcon className="size-4 group-hover:hidden" />
              <BellIcon className="hidden size-4 group-hover:block" />
              <span className="group-hover:hidden">{label}</span>
              <span className="hidden group-hover:inline">Unmute</span>
            </Button>
          </motion.div>
        ) : (
          <motion.div key="unmuted" initial={{ opacity: 0, scale: 0.9 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.9 }} transition={SPRING}>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button type="button" variant="outline" size="sm" className="group rounded-xl">
                  <BellOffIcon className="size-4 transition-transform group-hover:-rotate-12" /> Mute
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-52">
                {MUTE_FOR.map((m) => (
                  <DropdownMenuItem key={m.label} onSelect={() => onMute(m.ms === null ? null : new Date(Date.now() + m.ms))}>
                    {m.label}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
