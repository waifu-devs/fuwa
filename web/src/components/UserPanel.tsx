import { SettingsIcon, TvMinimalPlayIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useFuwa } from "@/fuwa/store";
import { ConnDot, UserAvatar, connectionLabel } from "@/components/Icons";
import { MuteButtons } from "@/components/calls/parts";
import { SPRING, SwapText } from "@/components/motion";
import { Private } from "@/components/Private";
import { displayName, shownStatus } from "@/lib/format";
import { comboLabel, actionById, bindingOf } from "@/lib/keybinds";
import { useNow } from "@/lib/notifications";
import { setPrefs, usePrefs } from "@/lib/prefs";
import { openSettings } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** You, on this instance, at the bottom of the sidebar. */
export function UserPanel({ instanceKey }: { instanceKey: string }) {
  const me = useFuwa((s) => s.instances[instanceKey]?.me);
  const connection = useFuwa((s) => s.instances[instanceKey]?.connection ?? "connecting");
  const streamer = usePrefs((p) => p.streamer);
  const streamerKey = usePrefs((p) => bindingOf(actionById("toggleStreamer")!, p));
  const now = useNow(60_000);
  if (!me) return null;
  return (
    <div className="flex items-center gap-0.5 border-t bg-[color-mix(in_srgb,var(--background)_50%,transparent)] p-2">
      <button
        type="button"
        onClick={() => openSettings("profile")}
        className="group flex min-w-0 flex-1 items-center gap-2 rounded-xl p-1 text-left transition hover:bg-muted"
      >
        <span className="relative shrink-0">
          <UserAvatar user={me} className="size-8 transition duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:-rotate-6 group-hover:scale-110" />
          <ConnDot state={connection} className="absolute -right-0.5 -bottom-0.5 ring-[3px] ring-card" />
        </span>
        <span className="min-w-0">
          <span className="block truncate text-sm font-bold">
            <SwapText className="truncate align-bottom">{displayName(me)}</SwapText>
          </span>
          <span className="block truncate text-xs text-muted-foreground">
            {connection === "live" ? (
              shownStatus(me, now) ? (
                <SwapText className="truncate align-bottom">{shownStatus(me, now)}</SwapText>
              ) : (
                <>
                  @<Private text={me.username} kind="name" />
                </>
              )
            ) : (
              <SwapText className="truncate align-bottom">{connectionLabel(connection)}</SwapText>
            )}
          </span>
        </span>
      </button>
      <MuteButtons />
      <button
        type="button"
        onClick={() => setPrefs({ streamer: !streamer })}
        aria-pressed={streamer}
        aria-label={streamer ? "Turn off streamer mode" : "Turn on streamer mode"}
        title={`${streamer ? "Turn off" : "Turn on"} streamer mode${streamerKey ? ` (${comboLabel(streamerKey)})` : ""}`}
        className={cn(
          "group relative grid size-8 place-items-center rounded-lg transition hover:bg-muted active:scale-90",
          streamer ? "text-primary" : "text-muted-foreground hover:text-foreground",
        )}
      >
        <motion.span key={String(streamer)} initial={{ scale: 0.6, rotate: -15 }} animate={{ scale: 1, rotate: 0 }} transition={SPRING}>
          <TvMinimalPlayIcon className="size-[18px]" />
        </motion.span>
        <AnimatePresence>
          {streamer && (
            <motion.span
              initial={{ scale: 0 }}
              animate={{ scale: 1 }}
              exit={{ scale: 0 }}
              transition={{ type: "spring", stiffness: 600, damping: 16 }}
              className="absolute top-1.5 right-1.5 size-2 rounded-full bg-destructive ring-2 ring-card"
            >
              <span className="absolute inset-0 animate-ping rounded-full bg-destructive" />
            </motion.span>
          )}
        </AnimatePresence>
      </button>
      <button
        type="button"
        onClick={() => openSettings()}
        aria-label="Settings"
        className="group grid size-8 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground"
      >
        <SettingsIcon className="size-[18px] transition-transform duration-500 group-hover:rotate-180" />
      </button>
    </div>
  );
}
