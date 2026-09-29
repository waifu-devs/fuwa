import { AtSignIcon, BellRingIcon, MessagesSquareIcon, PlayIcon, TvMinimalPlayIcon, Volume2Icon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";
import { FuwaMark } from "@/components/Icons";
import { Count, SPRING } from "@/components/motion";
import { Choice, Toggle } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { Slider } from "@/components/ui/slider";
import { testNotification } from "@/lib/notify";
import { setPrefs, usePrefs, type NotifyFor, type Sound } from "@/lib/prefs";
import { play } from "@/lib/sounds";
import { openSettings } from "@/lib/ui";
import { PrefSetting } from "./common";

const supported = typeof Notification !== "undefined";

const SOUNDS: { sound: Sound; label: string; hint: string }[] = [
  { sound: "message", label: "New message", hint: "In a channel you aren't looking at." },
  { sound: "mention", label: "Mention", hint: "Someone wrote your @username." },
  { sound: "join", label: "Someone joins", hint: "In the server you have open." },
];

export function Notifications() {
  const p = usePrefs((x) => x);
  const [permission, setPermission] = useState(supported ? Notification.permission : "denied");
  const streamerMutes = p.streamer && (p.streamerMuteNotifications || p.streamerMuteSounds);

  async function turnOn(on: boolean) {
    if (!on) return setPrefs({ desktopNotifications: false });
    if (!supported) return;
    const result = Notification.permission === "default" ? await Notification.requestPermission() : Notification.permission;
    setPermission(result);
    setPrefs({ desktopNotifications: result === "granted" });
  }

  return (
    <div className="flex flex-col">
      <AnimatePresence initial={false}>
        {streamerMutes && (
          <motion.button
            type="button"
            onClick={() => openSettings("streamer")}
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={SPRING}
            className="mb-4 flex items-center gap-2 overflow-hidden rounded-xl bg-primary/10 px-3 py-2 text-left text-sm text-primary"
          >
            <TvMinimalPlayIcon className="size-4 shrink-0" />
            Streamer mode is on and keeps {p.streamerMuteNotifications && p.streamerMuteSounds ? "notifications and sounds" : p.streamerMuteSounds ? "sounds" : "notifications"} quiet.
          </motion.button>
        )}
      </AnimatePresence>
      <PrefSetting id="desktop-notifications" title="Desktop notifications" keys={["desktopNotifications"]}>
        <Toggle
          checked={p.desktopNotifications && permission === "granted"}
          onChange={(on) => void turnOn(on)}
          disabled={!supported}
          label="Notify me while fuwa isn't in front"
          hint={
            !supported
              ? "This browser can't show notifications."
              : permission === "denied"
                ? "Your browser blocks notifications for this site. Allow them in the site's settings, then switch this on."
                : "Your browser asks for permission the first time."
          }
        />
        <AnimatePresence initial={false}>
          {p.desktopNotifications && permission === "granted" && (
            <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
              <Button type="button" variant="outline" size="sm" className="group rounded-xl" onClick={() => testNotification()}>
                <BellRingIcon className="transition-transform group-hover:rotate-12" /> Send a test
              </Button>
            </motion.div>
          )}
        </AnimatePresence>
      </PrefSetting>
      <PrefSetting id="notify-for" title="Notify me about" keys={["notifyFor"]} delay={0.04}>
        <Choice<NotifyFor>
          value={p.notifyFor}
          onChange={(notifyFor) => setPrefs({ notifyFor })}
          options={[
            { value: "mentions", label: "Mentions", hint: "Only when someone writes your @username.", icon: <AtSignIcon className="size-4" /> },
            { value: "all", label: "Every message", hint: "From every server you're in.", icon: <MessagesSquareIcon className="size-4" /> },
          ]}
        />
      </PrefSetting>
      <PrefSetting id="unread-badge" title="Unread count on the tab" keys={["unreadBadge"]} delay={0.08}>
        <Toggle
          checked={p.unreadBadge}
          onChange={(unreadBadge) => setPrefs({ unreadBadge })}
          label="Show unread messages in the tab's title and icon"
          hint="So you can tell from another tab."
        />
        <TabPreview on={p.unreadBadge} />
      </PrefSetting>
      <PrefSetting id="sounds" title="Sounds" keys={["sounds", "volume"]} delay={0.12}>
        <div className="flex flex-col gap-3">
          {SOUNDS.map(({ sound, label, hint }) => (
            <div key={sound} className="flex items-center gap-3">
              <button
                type="button"
                onClick={() => play(sound, true)}
                aria-label={`Play the ${label.toLowerCase()} sound`}
                className="group grid size-9 shrink-0 place-items-center rounded-full bg-muted text-muted-foreground transition hover:bg-primary hover:text-primary-foreground active:scale-90"
              >
                <PlayIcon className="size-4 translate-x-px transition-transform group-hover:scale-110" />
              </button>
              <div className="min-w-0 flex-1">
                <Toggle checked={p.sounds[sound]} onChange={(on) => setPrefs((x) => ({ sounds: { ...x.sounds, [sound]: on } }))} label={label} hint={hint} />
              </div>
            </div>
          ))}
          <div className="flex items-center gap-3 pt-1">
            <Volume2Icon className="size-5 shrink-0 text-muted-foreground" />
            <Slider
              label="Volume"
              className="flex-1"
              value={p.volume}
              min={0}
              max={100}
              step={5}
              format={(n) => `${n}%`}
              onChange={(volume) => setPrefs({ volume })}
              onCommit={() => play("message", true)}
            />
          </div>
        </div>
      </PrefSetting>
    </div>
  );
}

/** A browser tab with fuwa in it, showing where the count goes. */
function TabPreview({ on }: { on: boolean }) {
  return (
    <div className="flex items-end gap-1 rounded-xl bg-muted/60 px-3 pt-3">
      <div className="flex w-56 items-center gap-2 rounded-t-lg bg-background px-3 py-2 text-xs shadow-sm">
        <span className="relative shrink-0">
          <FuwaMark className="size-4" />
          <AnimatePresence>
            {on && (
              <motion.span
                initial={{ scale: 0 }}
                animate={{ scale: 1 }}
                exit={{ scale: 0 }}
                transition={{ type: "spring", stiffness: 600, damping: 16 }}
                className="absolute -top-1.5 -right-1.5 grid size-3 place-items-center rounded-full bg-destructive text-[0.5rem] font-extrabold text-white"
              >
                3
              </motion.span>
            )}
          </AnimatePresence>
        </span>
        <span className="truncate">
          <AnimatePresence initial={false}>
            {on && (
              <motion.b initial={{ opacity: 0, width: 0 }} animate={{ opacity: 1, width: "auto" }} exit={{ opacity: 0, width: 0 }} className="inline-block overflow-hidden align-bottom whitespace-nowrap">
                (<Count value={3} />)&nbsp;
              </motion.b>
            )}
          </AnimatePresence>
          #general · Waifu Devs
        </span>
      </div>
      <div className="w-24 rounded-t-lg bg-background/40 px-3 py-2 text-xs text-muted-foreground">Other tab</div>
    </div>
  );
}
