import { BellIcon, BellOffIcon, SettingsIcon } from "lucide-react";
import { motion, useAnimationControls } from "motion/react";
import { useEffect, useRef } from "react";
import { NotificationLevel, type Channel } from "@/gen/fuwa/v1/types_pb";
import { run, updateNotifications, type NotificationPatch } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useFuwa } from "@/fuwa/store";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useI18n } from "@/i18n/react";
import { isMuted, LEVELS, MUTE_FOR, muteForLabel, mutedHint, mutedLabel, mutedUntil, useNotificationSettings, useNow } from "@/lib/notifications";
import { openSettings, toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * The bell in a channel's header: mute the channel for a while, pick what
 * it notifies you about, or open the server's notification settings. It
 * rings when the channel gets muted or unmuted.
 */
export function NotificationBell({ instanceKey, serverId, channel }: { instanceKey: string; serverId: string; channel: Channel }) {
  const { t } = useI18n();
  const now = useNow();
  const server = useNotificationSettings(instanceKey, serverId);
  const serverUntil = mutedUntil(server, now);
  const settings = useNotificationSettings(instanceKey, serverId, channel.id);
  const channelMuted = isMuted(settings, now);
  const serverMuted = isMuted(server, now);
  const muted = channelMuted || serverMuted;
  const ring = useAnimationControls();
  const was = useRef(muted);

  useEffect(() => {
    if (was.current === muted) return;
    was.current = muted;
    void ring.start({ rotate: [0, -22, 18, -12, 8, -4, 0], transition: { duration: 0.6 } });
  }, [muted, ring]);

  const change = (patch: NotificationPatch) =>
    run(updateNotifications(instanceKey, serverId, channel.id, patch)).catch((err: FuwaError) => toast(err.message));

  const level = settings?.level ?? NotificationLevel.UNSPECIFIED;
  const serverDefault = useFuwa((s) => s.instances[instanceKey]?.servers.find((sv) => sv.id === serverId)?.defaultNotifications);
  const serverLevelKey = LEVELS.find((l) => l.value === server?.level)?.label;
  const serverLevel = serverLevelKey
    ? t(serverLevelKey)
    : serverDefault === NotificationLevel.MENTIONS
      ? "Only @mentions, the server's default"
      : undefined;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <motion.button
          type="button"
          whileTap={{ scale: 0.85 }}
          aria-label={muted ? `Notifications for #${channel.name}: ${serverMuted && !channelMuted ? "server muted" : "muted"}` : `Notifications for #${channel.name}`}
          title={muted ? (channelMuted ? mutedLabel(t, settings, now) : serverUntil ? t("common.notify.serverMutedUntil", { time: serverUntil }) : t("common.notify.serverMuted")) : "Notification settings"}
          className={cn(
            "grid size-9 place-items-center rounded-full transition-colors hover:bg-muted data-[state=open]:bg-muted",
            muted ? "text-amber-500" : "text-muted-foreground",
          )}
        >
          <motion.span animate={ring} style={{ originY: 0.1 }}>
            {muted ? <BellOffIcon className="size-5" /> : <BellIcon className="size-5" />}
          </motion.span>
        </motion.button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-64">
        <DropdownMenuLabel className="truncate text-xs text-muted-foreground">#{channel.name}</DropdownMenuLabel>
        {channelMuted ? (
          <DropdownMenuItem onSelect={() => void change({ mutedUntil: false })}>
            <BellIcon /> Unmute channel
            <span className="ml-auto truncate pl-2 text-xs text-muted-foreground">{mutedHint(t, settings, now)}</span>
          </DropdownMenuItem>
        ) : (
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>
              <BellOffIcon /> Mute channel
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent className="w-52">
              {MUTE_FOR.map((m) => (
                <DropdownMenuItem key={m.id} onSelect={() => void change({ mutedUntil: m.ms === null ? null : new Date(Date.now() + m.ms) })}>
                  {muteForLabel(t, m)}
                </DropdownMenuItem>
              ))}
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        )}
        {serverMuted && (
          <p className="px-2 pb-1 text-xs text-amber-500">
            {serverUntil ? t("common.notify.wholeServerMutedUntil", { time: serverUntil }) : t("common.notify.wholeServerMuted")}
          </p>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuRadioGroup value={String(level)} onValueChange={(v) => void change({ level: Number(v) as NotificationLevel })}>
          <DropdownMenuRadioItem value={String(NotificationLevel.UNSPECIFIED)}>
            <span className="min-w-0">
              <span className="block">Use the server's</span>
              <span className="block text-xs text-muted-foreground">{serverLevel ?? "This device decides"}</span>
            </span>
          </DropdownMenuRadioItem>
          {LEVELS.map((l) => (
            <DropdownMenuRadioItem key={l.value} value={String(l.value)}>
              {t(l.label)}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => openSettings("server-notifications", serverId)}>
          <SettingsIcon /> Notification settings
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
