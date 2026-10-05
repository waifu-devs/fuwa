import * as Popover from "@radix-ui/react-popover";
import { CircleDotIcon, HeadphoneOffIcon, HeadphonesIcon, MicIcon, MicOffIcon, MonitorUpIcon, PhoneOffIcon, ServerIcon, ShieldOffIcon, VideoIcon, VideoOffIcon, Volume2Icon, VolumeXIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState, type ReactNode } from "react";
import type { User, VoiceState } from "@/gen/fuwa/v1/types_pb";
import { Permission } from "@/gen/fuwa/v1/types_pb";
import { applyVolumes, toggleDeafen, toggleMute } from "@/calls/engine";
import { useCalls } from "@/calls/state";
import { toFuwaError } from "@/fuwa/errors";
import { useAccess, useInstance } from "@/fuwa/hooks";
import { engine } from "@/fuwa/sync";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { Slider } from "@/components/ui/slider";
import { displayName, memberName } from "@/lib/format";
import { actionById, bindingOf, comboLabel } from "@/lib/keybinds";
import { hasIn, outranks, standing } from "@/lib/permissions";
import { setPrefs, usePrefs } from "@/lib/prefs";
import { toast } from "@/lib/ui";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";
import { type Key, useI18n } from "@/i18n/react";

/** Whether someone's talking in your call right now. */
export const useSpeaking = (userId: string | undefined) => useCalls((s) => !!userId && !!s.speaking[userId]);

/**
 * Someone's avatar in a call: a soft ring breathes around it while they
 * talk, and fades when they stop.
 */
export function VoiceAvatar({ user, speaking, className, ring = 3 }: { user: User | undefined; speaking: boolean; className?: string; ring?: number }) {
  return (
    <span className="relative inline-grid shrink-0 place-items-center">
      <AnimatePresence>
        {speaking && (
          <motion.span
            key="ring"
            aria-hidden
            initial={{ opacity: 0, scale: 0.85 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.9 }}
            transition={{ type: "spring", stiffness: 700, damping: 26 }}
            className="speaking-ring absolute rounded-full"
            style={{ inset: -ring, borderWidth: ring }}
          />
        )}
      </AnimatePresence>
      <UserAvatar user={user} className={className} />
    </span>
  );
}

/** The little icons after someone's name: camera on, muted, deafened, or muted by a moderator (in red). */
export function VoiceFlags({ state, className }: { state: Pick<VoiceState, "selfMute" | "selfDeaf" | "serverMute" | "serverDeaf" | "suppress" | "selfVideo" | "selfStream" | "selfRecord" | "serverRecord" | "serverVideoOff"> | undefined; className?: string }) {
  const { t } = useI18n();
  if (!state) return null;
  const flags: { key: string; icon: typeof MicOffIcon; label: Key; mod?: boolean }[] = [];
  if (state.serverRecord) flags.push({ key: "server-record", icon: ServerIcon, label: "dms-calls.calls.flags.serverRecord", mod: true });
  if (state.selfRecord) flags.push({ key: "record", icon: CircleDotIcon, label: "dms-calls.calls.flags.record", mod: true });
  if (state.selfStream) flags.push({ key: "screen", icon: MonitorUpIcon, label: "dms-calls.calls.flags.screen" });
  if (state.selfVideo) flags.push({ key: "video", icon: VideoIcon, label: "dms-calls.calls.flags.video" });
  if (state.serverVideoOff) flags.push({ key: "server-video-off", icon: VideoOffIcon, label: "dms-calls.calls.flags.serverVideoOff", mod: true });
  if (state.serverMute) flags.push({ key: "server-mute", icon: MicOffIcon, label: "dms-calls.calls.flags.serverMute", mod: true });
  else if (state.suppress) flags.push({ key: "suppress", icon: MicOffIcon, label: "dms-calls.calls.flags.suppress", mod: true });
  else if (state.selfMute) flags.push({ key: "mute", icon: MicOffIcon, label: "dms-calls.calls.flags.mute" });
  if (state.serverDeaf) flags.push({ key: "server-deaf", icon: HeadphoneOffIcon, label: "dms-calls.calls.flags.serverDeaf", mod: true });
  else if (state.selfDeaf) flags.push({ key: "deaf", icon: HeadphoneOffIcon, label: "dms-calls.calls.flags.deaf" });
  return (
    <span className={cn("ml-auto flex shrink-0 items-center gap-0.5", className)}>
      <AnimatePresence initial={false}>
        {flags.map(({ key, icon: Icon, label, mod }) => (
          <motion.span
            key={key}
            title={t(label)}
            aria-label={t(label)}
            initial={{ scale: 0, rotate: -40 }}
            animate={{ scale: 1, rotate: 0 }}
            exit={{ scale: 0, rotate: 40 }}
            transition={{ type: "spring", stiffness: 650, damping: 18 }}
            className={cn("grid size-4 place-items-center", mod ? "text-destructive" : "text-muted-foreground")}
          >
            <Icon className="size-3.5" />
          </motion.span>
        ))}
      </AnimatePresence>
    </span>
  );
}


/** Mute and deafen, for the user panel and the call controls. They work in and out of calls, as Discord's do. */
export function MuteButtons({ size = "sm" }: { size?: "sm" | "lg" }) {
  const selfMute = useCalls((s) => s.selfMute);
  const selfDeaf = useCalls((s) => s.selfDeaf);
  const muteKey = usePrefs((p) => bindingOf(actionById("toggleMute")!, p));
  const deafKey = usePrefs((p) => bindingOf(actionById("toggleDeafen")!, p));
  const { t } = useI18n();
  /** A button's name, with its keyboard shortcut when it has one. */
  const named = (action: Key, combo: string | null) => (combo ? t("dms-calls.calls.controls.withShortcut", { action: t(action), keys: comboLabel(combo) }) : t(action));
  return (
    <>
      <ToggleIcon
        size={size}
        on={selfMute}
        onClick={toggleMute}
        label={named(selfMute ? "dms-calls.calls.controls.unmute" : "dms-calls.calls.controls.mute", muteKey)}
        icon={selfMute ? MicOffIcon : MicIcon}
        wiggle="mic"
      />
      <ToggleIcon
        size={size}
        on={selfDeaf}
        onClick={toggleDeafen}
        label={named(selfDeaf ? "dms-calls.calls.controls.undeafen" : "dms-calls.calls.controls.deafen", deafKey)}
        icon={selfDeaf ? HeadphoneOffIcon : HeadphonesIcon}
        wiggle="phones"
      />
    </>
  );
}

function ToggleIcon({ on, onClick, label, icon: Icon, size, wiggle }: { on: boolean; onClick: () => void; label: string; icon: typeof MicIcon; size: "sm" | "lg"; wiggle: "mic" | "phones" }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={on}
      aria-label={label}
      title={label}
      className={cn(
        "group relative grid shrink-0 place-items-center transition active:scale-90",
        size === "sm" ? "size-8 rounded-lg" : "size-12 rounded-2xl",
        on ? "bg-destructive/12 text-destructive hover:bg-destructive/20" : size === "sm" ? "text-muted-foreground hover:bg-muted hover:text-foreground" : "bg-muted text-foreground hover:bg-muted/70",
      )}
    >
      <motion.span
        key={String(on)}
        initial={{ scale: 0.5, rotate: wiggle === "mic" ? -25 : 25 }}
        animate={{ scale: 1, rotate: 0 }}
        transition={{ type: "spring", stiffness: 600, damping: 14 }}
        className="grid place-items-center"
      >
        <Icon className={cn(size === "sm" ? "size-[18px]" : "size-5", "transition-transform group-hover:scale-110")} />
      </motion.span>
    </button>
  );
}

/** Hang up, big and red. */
export function HangUpButton({ onClick, size = "sm", label: given }: { onClick: () => void; size?: "sm" | "lg"; label?: string }) {
  const { t } = useI18n();
  const label = given ?? t("dms-calls.calls.controls.disconnect");
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      className={cn(
        "group grid shrink-0 place-items-center bg-destructive text-white shadow-sm transition hover:brightness-110 active:scale-90",
        size === "sm" ? "size-9 rounded-xl" : "h-12 w-16 rounded-2xl",
      )}
    >
      <PhoneOffIcon className={cn(size === "sm" ? "size-[18px]" : "size-5", "transition-transform duration-300 group-hover:rotate-[135deg]")} />
    </button>
  );
}

/**
 * What you can do about someone in a call: how loud they are for you, and,
 * with Mute members or Move members there, mute, deafen or disconnect them for everyone.
 */
export function ParticipantMenu({
  instanceKey,
  serverId,
  channelId,
  user,
  state,
  children,
}: {
  instanceKey: string;
  /** Voice channels only: moderation happens in a server. */
  serverId?: string;
  channelId?: string;
  user: User | undefined;
  state?: VoiceState;
  children: ReactNode;
}) {
  const inst = useInstance(instanceKey);
  const me = inst?.me?.id;
  const userId = user?.id ?? "";
  const key = `${instanceKey}/${userId}`;
  const volume = usePrefs((p) => p.userVolumes[key] ?? 100);
  const access = useAccess(instanceKey, serverId ?? "");
  const server = inst?.servers.find((s) => s.id === serverId);
  const member = serverId ? inst?.members[serverId]?.find((m) => m.user?.id === userId) : undefined;
  const ranked = server ? outranks(access, standing(server.ownerId, inst?.roles[serverId!] ?? [], member)) : false;
  const canMute = !!channelId && ranked && hasIn(access, channelId, Permission.MUTE_MEMBERS);
  const canMove = !!channelId && ranked && hasIn(access, channelId, Permission.MOVE_MEMBERS);
  const [open, setOpen] = useState(false);
  const narrow = useMediaQuery("(max-width: 640px)");
  const { t, number } = useI18n();
  const self = userId === me;

  const setVolume = (v: number) => {
    setPrefs((p) => {
      const { [key]: _, ...rest } = p.userVolumes;
      return { userVolumes: v === 100 ? rest : { ...rest, [key]: v } };
    });
    applyVolumes();
  };

  const moderate = async (change: { serverMute?: boolean; serverDeaf?: boolean; serverVideoOff?: boolean; disconnect?: boolean }) => {
    try {
      await engine(instanceKey).api.calls.moderateVoice({ serverId: serverId!, userId, ...change });
      if (change.disconnect) setOpen(false);
    } catch (err) {
      toast(toFuwaError(err).message);
    }
  };

  if (self) return <>{children}</>;
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>{children}</Popover.Trigger>
      <AnimatePresence>
        {open && (
          <Popover.Portal forceMount>
            <Popover.Content asChild side={narrow ? "bottom" : "right"} align={narrow ? "center" : "start"} sideOffset={8} collisionPadding={12} forceMount>
              <motion.div
                initial={{ opacity: 0, scale: 0.92, x: -6 }}
                animate={{ opacity: 1, scale: 1, x: 0 }}
                exit={{ opacity: 0, scale: 0.95, x: -4 }}
                transition={SPRING}
                className="z-50 w-64 origin-[var(--radix-popover-content-transform-origin)] rounded-2xl border bg-popover p-3 text-popover-foreground shadow-xl"
              >
                <div className="mb-3 flex items-center gap-2">
                  <UserAvatar user={user} className="size-8" />
                  <span className="min-w-0 flex-1 truncate text-sm font-bold">{member ? memberName(member) : displayName(user)}</span>
                </div>
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    onClick={() => setVolume(volume === 0 ? 100 : 0)}
                    aria-label={volume === 0 ? t("dms-calls.calls.participant.unmuteForMe") : t("dms-calls.calls.participant.muteForMe")}
                    title={volume === 0 ? t("dms-calls.calls.participant.unmuteForMe") : t("dms-calls.calls.participant.muteForMe")}
                    className="grid size-8 shrink-0 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90"
                  >
                    {volume === 0 ? <VolumeXIcon className="size-4 text-destructive" /> : <Volume2Icon className="size-4" />}
                  </button>
                  <Slider
                    label={t("dms-calls.calls.participant.volume")}
                    className="flex-1"
                    value={volume}
                    min={0}
                    max={200}
                    step={5}
                    format={(n) => number(n / 100, { style: "percent" })}
                    marks={[{ value: 100, label: number(1, { style: "percent" }) }]}
                    onChange={setVolume}
                  />
                </div>
                {(canMute || canMove) && state && (
                  <div className="mt-3 flex flex-col gap-1 border-t pt-2">
                    {canMute && (
                      <ModItem on={state.serverMute} onClick={() => void moderate({ serverMute: !state.serverMute })} icon={MicOffIcon}>
                        {state.serverMute ? t("dms-calls.calls.participant.unmuteAll") : t("dms-calls.calls.participant.muteAll")}
                      </ModItem>
                    )}
                    {canMute && (
                      <ModItem on={state.serverDeaf} onClick={() => void moderate({ serverDeaf: !state.serverDeaf })} icon={HeadphoneOffIcon}>
                        {state.serverDeaf ? t("dms-calls.calls.participant.undeafenAll") : t("dms-calls.calls.participant.deafenAll")}
                      </ModItem>
                    )}
                    {canMute && (
                      <ModItem on={state.serverVideoOff} onClick={() => void moderate({ serverVideoOff: !state.serverVideoOff })} icon={VideoOffIcon}>
                        {t(
                          state.serverVideoOff
                            ? "dms-calls.calls.participant.allowVideo"
                            : state.selfStream && !state.selfVideo
                              ? "dms-calls.calls.participant.stopScreen"
                              : state.selfVideo && !state.selfStream
                                ? "dms-calls.calls.participant.stopCamera"
                                : "dms-calls.calls.participant.stopVideo",
                        )}
                      </ModItem>
                    )}
                    {canMove && (
                      <ModItem danger onClick={() => void moderate({ disconnect: true })} icon={ShieldOffIcon}>
                        {t("dms-calls.calls.participant.disconnect")}
                      </ModItem>
                    )}
                  </div>
                )}
              </motion.div>
            </Popover.Content>
          </Popover.Portal>
        )}
      </AnimatePresence>
    </Popover.Root>
  );
}

function ModItem({ on, danger, onClick, icon: Icon, children }: { on?: boolean; danger?: boolean; onClick: () => void; icon: typeof MicOffIcon; children: ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "group flex items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm font-bold transition hover:bg-muted active:scale-[0.98]",
        (danger || on) && "text-destructive",
      )}
    >
      <Icon className="size-4 transition-transform group-hover:-rotate-12 group-hover:scale-110" />
      {children}
    </button>
  );
}
