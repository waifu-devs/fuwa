import { BellIcon, BellOffIcon, SettingsIcon } from "lucide-react";
import { m as motion, useAnimationControls } from "motion/react";
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
import { type I18n, useI18n } from "@/i18n/react";
import { isMuted, LEVELS, MUTE_FOR, muteForLabel, mutedHint, mutedLabel, mutedUntil, useNotificationSettings, useNow } from "@/lib/notifications";
import { openSettings, toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Rings the bell whenever `muted` flips. */
function useRing(muted: boolean) {
  const ring = useAnimationControls();
  const was = useRef(muted);

  useEffect(() => {
    if (was.current === muted) return;
    was.current = muted;
    void ring.start({ rotate: [0, -22, 18, -12, 8, -4, 0], transition: { duration: 0.6 } });
  }, [muted, ring]);

  return ring;
}

/** What the bell says to screen readers: the channel, and whether it (or its whole server) is muted. */
function bellLabel(t: I18n["t"], channel: string, channelMuted: boolean, serverMuted: boolean) {
  if (channelMuted) return t("chat.bell.labelMuted", { channel });
  if (serverMuted) return t("chat.bell.labelServerMuted", { channel });
  return t("chat.bell.label", { channel });
}

/** The bell's tooltip: how long the channel's muted (`channelMutedLabel`), else the server's, else what it opens. */
function bellTitle(t: I18n["t"], channelMutedLabel: string | null, serverMuted: boolean, serverUntil: string) {
  if (channelMutedLabel !== null) return channelMutedLabel;
  if (!serverMuted) return t("chat.bell.settings");
  return serverUntil ? t("common.notify.serverMutedUntil", { time: serverUntil }) : t("common.notify.serverMuted");
}

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
  const ring = useRing(muted);

  const change = (patch: NotificationPatch) =>
    run(updateNotifications(instanceKey, serverId, channel.id, patch)).catch((err: FuwaError) => toast(err.message));

  const level = settings?.level ?? NotificationLevel.UNSPECIFIED;
  const serverDefault = useFuwa((s) => s.instances[instanceKey]?.servers.find((sv) => sv.id === serverId)?.defaultNotifications);
  const serverLevelKey = LEVELS.find((l) => l.value === server?.level)?.label;
  const mentionsByDefault = serverDefault === NotificationLevel.MENTIONS;
  const serverLevel = serverLevelKey ? t(serverLevelKey) : mentionsByDefault ? t("chat.bell.mentionsDefault") : undefined;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <motion.button
          type="button"
          whileTap={{ scale: 0.85 }}
          aria-label={bellLabel(t, channel.name, channelMuted, serverMuted)}
          title={bellTitle(t, channelMuted ? mutedLabel(t, settings, now) : null, serverMuted, serverUntil)}
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
            <BellIcon /> {t("chat.bell.unmute")}
            <span className="ml-auto truncate pl-2 text-xs text-muted-foreground">{mutedHint(t, settings, now)}</span>
          </DropdownMenuItem>
        ) : (
          <MuteFor onPick={(mutedUntil) => void change({ mutedUntil })} />
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
              <span className="block">{t("chat.bell.useServer")}</span>
              <span className="block text-xs text-muted-foreground">{serverLevel ?? t("chat.bell.deviceDecides")}</span>
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
          <SettingsIcon /> {t("chat.bell.settings")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** "Mute channel", for one of the usual stretches or until it's turned back on. */
function MuteFor({ onPick }: { onPick: (until: Date | null) => void }) {
  const { t } = useI18n();
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <BellOffIcon /> {t("chat.bell.mute")}
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent className="w-52">
        {MUTE_FOR.map((m) => (
          <DropdownMenuItem key={m.id} onSelect={() => onPick(m.ms === null ? null : new Date(Date.now() + m.ms))}>
            {muteForLabel(t, m)}
          </DropdownMenuItem>
        ))}
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );
}
