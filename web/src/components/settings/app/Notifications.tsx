import { AtSignIcon, BellRingIcon, MessagesSquareIcon, PlayIcon, TvMinimalPlayIcon, Volume2Icon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState } from "react";
import { FuwaMark } from "@/components/Icons";
import { Count, SPRING } from "@/components/motion";
import { Choice, Toggle } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { Slider } from "@/components/ui/slider";
import { type Key, useI18n } from "@/i18n/react";
import { testNotification } from "@/lib/notify";
import { setPrefs, usePrefs, type NotifyFor, type Sound } from "@/lib/prefs";
import { play } from "@/lib/sounds";
import { openSettings } from "@/lib/ui";
import { PrefSetting } from "./common";

const supported = typeof Notification !== "undefined";

const SOUNDS: { sound: Sound; label: Key; hint: Key; play: Key }[] = [
  { sound: "message", label: "appsettings.notifications.soundMessage", hint: "appsettings.notifications.soundMessageHint", play: "appsettings.notifications.playMessage" },
  { sound: "mention", label: "appsettings.notifications.soundMention", hint: "appsettings.notifications.soundMentionHint", play: "appsettings.notifications.playMention" },
  { sound: "join", label: "appsettings.notifications.soundJoin", hint: "appsettings.notifications.soundJoinHint", play: "appsettings.notifications.playJoin" },
];

export function Notifications() {
  const { t, number } = useI18n();
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
            {p.streamerMuteNotifications && p.streamerMuteSounds
              ? t("appsettings.notifications.streamerQuietBoth")
              : p.streamerMuteSounds
                ? t("appsettings.notifications.streamerQuietSounds")
                : t("appsettings.notifications.streamerQuietNotifications")}
          </motion.button>
        )}
      </AnimatePresence>
      <PrefSetting id="desktop-notifications" title={t("appsettings.notifications.desktop")} keys={["desktopNotifications"]}>
        <Toggle
          checked={p.desktopNotifications && permission === "granted"}
          onChange={(on) => void turnOn(on)}
          disabled={!supported}
          label={t("appsettings.notifications.desktopToggle")}
          hint={
            !supported
              ? t("appsettings.notifications.unsupported")
              : permission === "denied"
                ? t("appsettings.notifications.blocked")
                : t("appsettings.notifications.asks")
          }
        />
        <AnimatePresence initial={false}>
          {p.desktopNotifications && permission === "granted" && (
            <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
              <Button type="button" variant="outline" size="sm" className="group rounded-xl" onClick={() => testNotification()}>
                <BellRingIcon className="transition-transform group-hover:rotate-12" /> {t("appsettings.notifications.test")}
              </Button>
            </motion.div>
          )}
        </AnimatePresence>
      </PrefSetting>
      <PrefSetting id="notify-for" title={t("appsettings.notifications.notifyFor")} keys={["notifyFor"]} delay={0.04}>
        <Choice<NotifyFor>
          value={p.notifyFor}
          onChange={(notifyFor) => setPrefs({ notifyFor })}
          options={[
            { value: "mentions", label: t("appsettings.notifications.mentions"), hint: t("appsettings.notifications.mentionsHint"), icon: <AtSignIcon className="size-4" /> },
            { value: "all", label: t("appsettings.notifications.all"), hint: t("appsettings.notifications.allHint"), icon: <MessagesSquareIcon className="size-4" /> },
          ]}
        />
      </PrefSetting>
      <PrefSetting id="unread-badge" title={t("appsettings.notifications.unreadBadge")} keys={["unreadBadge"]} delay={0.08}>
        <Toggle
          checked={p.unreadBadge}
          onChange={(unreadBadge) => setPrefs({ unreadBadge })}
          label={t("appsettings.notifications.unreadBadgeToggle")}
          hint={t("appsettings.notifications.unreadBadgeToggleHint")}
        />
        <TabPreview on={p.unreadBadge} />
      </PrefSetting>
      <PrefSetting id="sounds" title={t("appsettings.notifications.sounds")} keys={["sounds", "volume"]} delay={0.12}>
        <div className="flex flex-col gap-3">
          {SOUNDS.map(({ sound, label, hint, play: playLabel }) => (
            <div key={sound} className="flex items-center gap-3">
              <button
                type="button"
                onClick={() => play(sound, true)}
                aria-label={t(playLabel)}
                className="group grid size-9 shrink-0 place-items-center rounded-full bg-muted text-muted-foreground transition hover:bg-primary hover:text-primary-foreground active:scale-90"
              >
                <PlayIcon className="size-4 translate-x-px transition-transform group-hover:scale-110" />
              </button>
              <div className="min-w-0 flex-1">
                <Toggle checked={p.sounds[sound]} onChange={(on) => setPrefs((x) => ({ sounds: { ...x.sounds, [sound]: on } }))} label={t(label)} hint={t(hint)} />
              </div>
            </div>
          ))}
          <div className="flex items-center gap-3 pt-1">
            <Volume2Icon className="size-5 shrink-0 text-muted-foreground" />
            <Slider
              label={t("appsettings.notifications.volume")}
              className="flex-1"
              value={p.volume}
              min={0}
              max={100}
              step={5}
              format={(n) => number(n / 100, { style: "percent" })}
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
  const { t, number } = useI18n();
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
                {number(3)}
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
          #{t("appsettings.preview.general")} · Waifu Devs
        </span>
      </div>
      <div className="w-24 rounded-t-lg bg-background/40 px-3 py-2 text-xs text-muted-foreground">{t("appsettings.notifications.otherTab")}</div>
    </div>
  );
}
