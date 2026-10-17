import * as Popover from "@radix-ui/react-popover";
import { AudioLinesIcon, LoaderCircleIcon, PlayIcon, PlusIcon, RotateCwIcon, SquareIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { Permission, type Emoji, type Sound } from "@/gen/fuwa/v1/types_pb";
import { useCalls } from "@/calls/state";
import { byId, useAccess } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { loadSounds, playSound, previewSound, useJustPlayed, useSounds } from "@/fuwa/soundboard";
import type { FuwaError } from "@/fuwa/errors";
import { EmojiImage } from "@/components/EmojiImage";
import { openMenuDialog } from "@/components/menus/dialogs";
import { instanceHas } from "@/lib/compat";
import { SPRING } from "@/lib/motion";
import { has, hasIn } from "@/lib/permissions";
import { playBlocked, readEmoji, type Blocked } from "@/lib/soundboard";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { type Key, useI18n } from "@/i18n/react";

/*
 * The soundboard in a voice channel's call (docs/soundboard.md): a button
 * with the call's controls that opens a grid of the server's sounds. A
 * press plays one for everyone (you hear it when the instance says it
 * played, like everyone else); the little play button previews it for you
 * alone. Direct-message calls have none.
 */

const BLOCKED: Record<Exclude<Blocked, null>, Key> = {
  connecting: "dms-calls.calls.soundboard.connecting",
  noSpeak: "dms-calls.calls.soundboard.noSpeak",
  serverMuted: "dms-calls.calls.soundboard.serverMuted",
  deafened: "dms-calls.calls.soundboard.deafened",
  muted: "dms-calls.calls.soundboard.muted",
};

/** A sound's emoji: a server's own as its picture, a Unicode one as it is, or a sound wave for none. */
export function SoundEmoji({ emoji, emojis, className }: { emoji: string; emojis: Emoji[] | undefined; className?: string }) {
  const read = readEmoji(emoji);
  if (read?.kind === "unicode") return <span className={cn("inline-grid place-items-center leading-none", className)}>{read.text}</span>;
  const custom = read && emojis?.find((e) => e.id === read.id);
  if (custom) return <EmojiImage emoji={custom} title={false} className={className} />;
  return <AudioLinesIcon className={cn("text-muted-foreground", className)} />;
}

/** The soundboard button, where the call you're in is a voice channel you may use it in, on an instance that has one. */
export function SoundboardButton({ size = "sm", className }: { size?: "sm" | "lg"; className?: string }) {
  const target = useCalls((s) => s.call?.target);
  const voice = target?.kind === "voice" ? target : null;
  const access = useAccess(voice?.instance ?? "", voice?.serverId ?? "");
  const here = useFuwa((s) => !!voice && instanceHas(s.instances[voice.instance]?.node?.versions, "soundboard"));
  const [open, setOpen] = useState(false);
  const { t } = useI18n();
  if (!voice || !here || !hasIn(access, voice.channelId, Permission.USE_SOUNDBOARD)) return null;
  const label = t("dms-calls.calls.soundboard.button");
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <button
          type="button"
          aria-label={label}
          title={label}
          aria-expanded={open}
          className={cn(
            "group relative grid shrink-0 place-items-center transition active:scale-90",
            size === "sm" ? "size-8 rounded-lg" : "size-12 rounded-2xl",
            open
              ? "bg-primary text-primary-foreground hover:brightness-110"
              : size === "sm"
                ? "text-muted-foreground hover:bg-muted hover:text-foreground"
                : "bg-muted text-foreground hover:bg-muted/70",
            className,
          )}
        >
          <motion.span key={String(open)} initial={{ scale: 0.5, rotate: -20 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 600, damping: 14 }} className="grid place-items-center">
            <AudioLinesIcon className={cn(size === "sm" ? "size-[18px]" : "size-5", "transition-transform group-hover:scale-110")} />
          </motion.span>
        </button>
      </Popover.Trigger>
      <AnimatePresence>
        {open && <SoundboardPanel key="panel" instanceKey={voice.instance} serverId={voice.serverId} channelId={voice.channelId} onClose={() => setOpen(false)} />}
      </AnimatePresence>
    </Popover.Root>
  );
}

/** The open soundboard: why it can't play when it can't, then the sounds. */
function SoundboardPanel({ instanceKey, serverId, channelId, onClose }: { instanceKey: string; serverId: string; channelId: string; onClose: () => void }) {
  const { t } = useI18n();
  const list = useSounds(instanceKey, serverId);
  const access = useAccess(instanceKey, serverId);
  const emojis = useFuwa((s) => s.instances[instanceKey]?.emojis[serverId]);
  const server = useFuwa((s) => byId(s.instances[instanceKey]?.servers, serverId));
  const me = useFuwa((s) => {
    const inst = s.instances[instanceKey];
    return inst?.voice[serverId]?.find((v) => v.userId === inst.me?.id);
  });
  const selfMute = useCalls((s) => s.selfMute);
  const selfDeaf = useCalls((s) => s.selfDeaf);
  const connected = useCalls((s) => s.call?.status === "connected");
  const blocked = playBlocked({
    connected,
    speak: hasIn(access, channelId, Permission.SPEAK),
    selfMute,
    selfDeaf,
    serverMute: !!me?.serverMute,
    serverDeaf: !!me?.serverDeaf,
    suppress: !!me?.suppress,
  });
  const manage = has(access, Permission.MANAGE_SOUNDBOARD);
  const sounds = list?.sounds ?? [];

  const openSettings = () => {
    if (!server) return;
    onClose();
    openMenuDialog({ kind: "server-settings", instanceKey, server, tab: "soundboard" });
  };

  return (
    <Popover.Portal forceMount>
      <Popover.Content asChild side="top" align="center" sideOffset={10} collisionPadding={12} forceMount>
        <motion.div
          initial={{ opacity: 0, scale: 0.92, y: 8 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.95, y: 6 }}
          transition={SPRING}
          className="z-50 flex max-h-[min(28rem,var(--radix-popover-content-available-height))] w-[min(22rem,calc(100vw-1.5rem))] origin-[var(--radix-popover-content-transform-origin)] flex-col rounded-2xl border bg-popover text-popover-foreground shadow-xl"
        >
          <div className="flex items-center gap-2 border-b px-3 py-2.5">
            <AudioLinesIcon className="size-4 text-primary" />
            <span className="flex-1 text-sm font-extrabold">{t("dms-calls.calls.soundboard.title")}</span>
            {manage && sounds.length > 0 && (
              <button
                type="button"
                onClick={openSettings}
                aria-label={t("dms-calls.calls.soundboard.openSettings")}
                title={t("dms-calls.calls.soundboard.openSettings")}
                className="grid size-7 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90"
              >
                <PlusIcon className="size-4 transition-transform hover:rotate-90" />
              </button>
            )}
          </div>
          <AnimatePresence initial={false} mode="popLayout">
            {blocked && sounds.length > 0 && (
              <motion.p
                key={blocked}
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: "auto" }}
                exit={{ opacity: 0, height: 0 }}
                transition={SPRING}
                className="overflow-hidden bg-amber-500/10 px-3 py-2 text-xs font-bold text-amber-600 dark:text-amber-400"
              >
                {t(BLOCKED[blocked])}
              </motion.p>
            )}
          </AnimatePresence>
          <div className="scroll-thin min-h-0 flex-1 overflow-y-auto p-2">
            {!list || (list.status === "loading" && !sounds.length) ? (
              <div className="grid grid-cols-3 gap-1.5" aria-label={t("dms-calls.calls.soundboard.loading")}>
                {[0, 1, 2, 3, 4, 5].map((n) => (
                  <div key={n} className="shimmer h-16 rounded-xl" />
                ))}
              </div>
            ) : list.status === "failed" && !sounds.length ? (
              <div className="flex flex-col items-center gap-2 py-6 text-center text-sm">
                <p className="text-muted-foreground">{t("dms-calls.calls.soundboard.failed")}</p>
                <button
                  type="button"
                  onClick={() => void loadSounds(instanceKey, serverId)}
                  className="group flex items-center gap-1.5 rounded-full bg-muted px-3 py-1 text-xs font-bold transition hover:bg-muted/70 active:scale-95"
                >
                  <RotateCwIcon className="size-3.5 transition-transform group-hover:rotate-90" /> {t("dms-calls.calls.soundboard.retry")}
                </button>
              </div>
            ) : !sounds.length ? (
              <motion.div initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col items-center gap-1.5 px-4 py-6 text-center">
                <motion.span animate={{ rotate: [0, -12, 12, -6, 0] }} transition={{ duration: 1.8, repeat: Infinity, repeatDelay: 1.4 }} className="text-3xl">
                  📯
                </motion.span>
                <p className="font-bold">{t("dms-calls.calls.soundboard.empty")}</p>
                <p className="text-xs text-muted-foreground">{t(manage ? "dms-calls.calls.soundboard.emptyManage" : "dms-calls.calls.soundboard.emptyText")}</p>
                {manage && (
                  <button
                    type="button"
                    onClick={openSettings}
                    className="group mt-1 flex items-center gap-1.5 rounded-full bg-primary px-3 py-1.5 text-xs font-bold text-primary-foreground transition hover:brightness-110 active:scale-95"
                  >
                    <PlusIcon className="size-3.5 transition-transform group-hover:rotate-90" /> {t("dms-calls.calls.soundboard.openSettings")}
                  </button>
                )}
              </motion.div>
            ) : (
              <>
                {!blocked && <p className="px-1 pb-2 text-xs text-muted-foreground">{t("dms-calls.calls.soundboard.hint")}</p>}
                <ul className="grid grid-cols-3 gap-1.5">
                  <AnimatePresence initial={false}>
                    {sounds.map((sound, n) => (
                      <SoundCell
                        key={sound.id}
                        index={n}
                        instanceKey={instanceKey}
                        serverId={serverId}
                        channelId={channelId}
                        sound={sound}
                        emojis={emojis}
                        disabled={!!blocked}
                        reason={blocked ? t(BLOCKED[blocked]) : undefined}
                      />
                    ))}
                  </AnimatePresence>
                </ul>
              </>
            )}
          </div>
        </motion.div>
      </Popover.Content>
    </Popover.Portal>
  );
}

/** One sound: press to play it for everyone, or preview it alone with the little button. */
function SoundCell({
  index,
  instanceKey,
  serverId,
  channelId,
  sound,
  emojis,
  disabled,
  reason,
}: {
  index: number;
  instanceKey: string;
  serverId: string;
  channelId: string;
  sound: Sound;
  emojis: Emoji[] | undefined;
  disabled: boolean;
  reason?: string;
}) {
  const { t } = useI18n();
  const [sending, setSending] = useState(false);
  const [previewing, setPreviewing] = useState(false);
  const stop = useRef<(() => void) | null>(null);
  useEffect(() => () => stop.current?.(), []);

  const play = async () => {
    if (disabled || sending) return;
    setSending(true);
    try {
      await playSound(instanceKey, serverId, channelId, sound.id);
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setSending(false);
    }
  };

  const preview = async () => {
    if (previewing) {
      stop.current?.();
      stop.current = null;
      return setPreviewing(false);
    }
    setPreviewing(true);
    try {
      stop.current = await previewSound(sound, () => setPreviewing(false));
    } catch (err) {
      setPreviewing(false);
      toast((err as Error).message);
    }
  };

  return (
    <motion.li
      layout
      initial={{ opacity: 0, scale: 0.85 }}
      animate={{ opacity: 1, scale: 1, transition: { ...SPRING, delay: Math.min(index, 12) * 0.02 } }}
      exit={{ opacity: 0, scale: 0.85 }}
      transition={SPRING}
      className="group/sound relative"
    >
      <motion.button
        type="button"
        onClick={() => void play()}
        aria-disabled={disabled}
        aria-label={t("dms-calls.calls.soundboard.play", { name: sound.name })}
        title={reason ?? t("dms-calls.calls.soundboard.play", { name: sound.name })}
        whileHover={disabled ? undefined : { y: -2 }}
        whileTap={disabled ? undefined : { scale: 0.92 }}
        className={cn(
          "flex h-16 w-full flex-col items-center justify-center gap-1 rounded-xl border bg-background/50 px-1.5 text-center transition-colors",
          disabled ? "cursor-not-allowed opacity-50" : "hover:border-primary/40 hover:bg-primary/5",
          sending && "border-primary/60 bg-primary/10",
        )}
      >
        <SoundEmoji emoji={sound.emoji} emojis={emojis} className="size-6 text-xl" />
        <span className="w-full truncate text-xs font-bold">{sound.name}</span>
      </motion.button>
      <button
        type="button"
        onClick={() => void preview()}
        aria-pressed={previewing}
        aria-label={t("dms-calls.calls.soundboard.preview", { name: sound.name })}
        title={t("dms-calls.calls.soundboard.preview", { name: sound.name })}
        className={cn(
          "absolute top-1 right-1 grid size-5 place-items-center rounded-full bg-background/80 text-muted-foreground shadow-sm transition hover:scale-110 hover:text-foreground active:scale-90",
          previewing ? "opacity-100" : "opacity-0 group-hover/sound:opacity-100 focus-visible:opacity-100 [@media(hover:none)]:opacity-100",
        )}
      >
        {previewing ? <SquareIcon className="size-2.5 fill-current" /> : <PlayIcon className="size-2.5 fill-current" />}
      </button>
      {sending && <LoaderCircleIcon className="absolute top-1 left-1 size-3.5 animate-spin text-primary" />}
    </motion.li>
  );
}

/** A bubble over someone's tile for a moment after they play a sound: its emoji and name. */
export function PlayedBubble({
  instanceKey,
  serverId,
  channelId,
  userId,
  name,
  className,
}: {
  instanceKey: string;
  serverId: string;
  channelId: string;
  userId: string;
  /** Who played it, as the tile names them. */
  name: string;
  className?: string;
}) {
  const { t } = useI18n();
  const played = useJustPlayed(instanceKey, serverId, channelId, userId);
  const emojis = useFuwa((s) => s.instances[instanceKey]?.emojis[serverId]);
  return (
    <AnimatePresence>
      {played && (
        <motion.span
          key={played.n}
          role="status"
          aria-label={t("dms-calls.calls.soundboard.played", { name, sound: played.sound.name })}
          title={t("dms-calls.calls.soundboard.played", { name, sound: played.sound.name })}
          initial={{ opacity: 0, scale: 0.6, y: 10 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.8, y: -12 }}
          transition={{ type: "spring", stiffness: 520, damping: 22 }}
          className={cn("pointer-events-none flex max-w-[85%] items-center gap-1.5 rounded-full bg-background/85 px-2.5 py-1 shadow-md backdrop-blur", className)}
        >
          <motion.span initial={{ rotate: -25, scale: 0.6 }} animate={{ rotate: [-25, 12, -6, 0], scale: [0.6, 1.25, 1] }} transition={{ duration: 0.6 }} className="grid place-items-center">
            <SoundEmoji emoji={played.sound.emoji} emojis={emojis} className="size-5 text-base" />
          </motion.span>
          <span className="truncate text-xs font-bold">{played.sound.name}</span>
        </motion.span>
      )}
    </AnimatePresence>
  );
}
