import { useRouterState } from "@tanstack/react-router";
import { KeyboardIcon, MonitorPlayIcon, TvMinimalPlayIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/lib/motion";
import { Private } from "@/components/Private";
import { Toggle, WithPreview } from "@/components/settings/controls";
import { Switch } from "@/components/ui/switch";
import { useInstance } from "@/fuwa/hooks";
import { T, useI18n } from "@/i18n/react";
import { actionById, bindingOf } from "@/lib/keybinds";
import { displayName } from "@/lib/format";
import { setPrefs, usePrefs } from "@/lib/prefs";
import { openSettings } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { Keycaps, PrefSetting } from "./common";

export function Streamer({ instanceKey }: { instanceKey?: string }) {
  const { t } = useI18n();
  const p = usePrefs((x) => x);
  const toggle = actionById("toggleStreamer")!;
  const combo = bindingOf(toggle, p);
  return (
    <WithPreview preview={<StreamPreview instanceKey={instanceKey} />}>
      <div className="flex flex-col">
        <PrefSetting id="streamer" title={t("appsettings.streamer.title")} keys={["streamer"]}>
          <label className="flex cursor-pointer items-center gap-4 rounded-2xl border p-4 transition-colors has-[[data-state=checked]]:border-primary/60 has-[[data-state=checked]]:bg-primary/5">
            <motion.span
              animate={p.streamer ? { rotate: [0, -10, 10, 0], scale: [1, 1.15, 1] } : { rotate: 0, scale: 1 }}
              transition={{ duration: 0.5 }}
              className={cn("grid size-11 shrink-0 place-items-center rounded-xl transition-colors", p.streamer ? "bg-primary text-primary-foreground" : "bg-muted text-muted-foreground")}
            >
              <TvMinimalPlayIcon className="size-5" />
            </motion.span>
            <span className="min-w-0 flex-1">
              <span className="block font-bold">{p.streamer ? t("appsettings.streamer.on") : t("appsettings.streamer.off")}</span>
              <span className="block text-sm text-muted-foreground">{t("appsettings.streamer.hint")}</span>
            </span>
            <Switch checked={p.streamer} onCheckedChange={(streamer) => setPrefs({ streamer })} />
          </label>
          <p className="flex flex-wrap items-center gap-1.5 text-sm text-muted-foreground">
            <KeyboardIcon className="size-4" />
            {combo ? (
              <T k="appsettings.streamer.shortcut" values={{ keys: <Keycaps combo={combo} /> }} />
            ) : (
              <>
                {t("appsettings.streamer.noShortcut")}
                <button type="button" onClick={() => openSettings("keybinds")} className="font-bold text-primary hover:underline">
                  {t("appsettings.streamer.addShortcut")}
                </button>
              </>
            )}
          </p>
        </PrefSetting>
        <PrefSetting id="streamer-hide-personal" title={t("appsettings.streamer.hidePersonal")} keys={["streamerHidePersonal"]} delay={0.04}>
          <Toggle
            checked={p.streamerHidePersonal}
            onChange={(streamerHidePersonal) => setPrefs({ streamerHidePersonal })}
            label={t("appsettings.streamer.hidePersonalToggle")}
            hint={t("appsettings.streamer.hidePersonalHint", { alias: "/~waifu-devs" })}
          />
        </PrefSetting>
        <PrefSetting id="streamer-sounds" title={t("appsettings.notifications.sounds")} keys={["streamerMuteSounds"]} delay={0.08}>
          <Toggle checked={p.streamerMuteSounds} onChange={(streamerMuteSounds) => setPrefs({ streamerMuteSounds })} label={t("appsettings.streamer.muteSounds")} hint={t("appsettings.streamer.muteSoundsHint")} />
        </PrefSetting>
        <PrefSetting id="streamer-notifications" title={t("settings.nav.notifications")} keys={["streamerMuteNotifications"]} delay={0.12}>
          <Toggle
            checked={p.streamerMuteNotifications}
            onChange={(streamerMuteNotifications) => setPrefs({ streamerMuteNotifications })}
            label={t("appsettings.streamer.muteNotifications")}
            hint={t("appsettings.streamer.muteNotificationsHint")}
          />
        </PrefSetting>
        <section data-setting="streamer-auto" className="flex items-center gap-4 py-6 opacity-70">
          <MonitorPlayIcon className="size-5 shrink-0 text-muted-foreground" />
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-bold">{t("appsettings.streamer.auto")}</span>
            <span className="block text-xs text-muted-foreground">{t("appsettings.streamer.autoHint")}</span>
          </span>
          <span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-[0.65rem] font-bold text-muted-foreground uppercase">{t("appsettings.streamer.desktopApp")}</span>
        </section>
      </div>
    </WithPreview>
  );
}

/** What a stream would see: the address, your name and a server's tooltip, live. */
function StreamPreview({ instanceKey }: { instanceKey?: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const href = useRouterState({ select: (s) => s.location.publicHref ?? s.location.href });
  const on = usePrefs((p) => p.streamer);
  const path = (href.split("?")[0] ?? "/").split("/").slice(0, 2).join("/") || "/";
  return (
    <div className="flex flex-col gap-3 rounded-3xl border bg-card p-4 shadow-lg">
      <div className="flex items-center gap-2">
        <span className="size-2.5 rounded-full bg-destructive/70" />
        <span className="size-2.5 rounded-full bg-amber-400/70" />
        <span className="size-2.5 rounded-full bg-emerald-400/70" />
        <AnimatePresence>
          {on && (
            <motion.span
              initial={{ opacity: 0, scale: 0.6 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.6 }}
              transition={SPRING}
              className="ml-auto flex items-center gap-1 rounded-full bg-destructive px-2 py-0.5 text-[0.6rem] font-extrabold text-white uppercase"
            >
              <span className="size-1.5 animate-pulse rounded-full bg-white" /> {t("appsettings.streamer.live")}
            </motion.span>
          )}
        </AnimatePresence>
      </div>
      <p className="truncate rounded-lg bg-muted px-2.5 py-1.5 font-mono text-xs text-muted-foreground">
        <motion.span key={path} initial={{ opacity: 0, filter: "blur(4px)" }} animate={{ opacity: 1, filter: "blur(0px)" }}>
          …{path}/…
        </motion.span>
      </p>
      {inst?.me && (
        <div className="flex items-center gap-2 rounded-xl bg-muted/50 p-2">
          <UserAvatar user={inst.me} className="size-8" />
          <span className="min-w-0 text-sm">
            <span className="block truncate font-bold">{displayName(inst.me)}</span>
            <span className="block truncate text-xs text-muted-foreground">
              @<Private text={inst.me.username} kind="name" />
            </span>
          </span>
        </div>
      )}
      {inst && (
        <div className="self-start rounded-lg bg-popover px-2.5 py-1.5 text-xs font-bold shadow-md">
          {inst.node?.name ?? t("appsettings.streamer.instance")} · <Private text={inst.key} />
        </div>
      )}
    </div>
  );
}
