import { create } from "@bufbuild/protobuf";
import { AlignJustifyIcon, MessageSquareTextIcon, MonitorIcon, Rows2Icon, Rows3Icon, Rows4Icon, SnailIcon, SparklesIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { UserSchema } from "@/gen/fuwa/v1/types_pb";
import { MessageBody, MessageLine } from "@/components/chat/MessageList";
import { SPRING } from "@/components/motion";
import { Choice, Toggle, WithPreview } from "@/components/settings/controls";
import { Slider } from "@/components/ui/slider";
import { useInstance } from "@/fuwa/hooks";
import { useI18n } from "@/i18n/react";
import { allThemes, darkThemes, lightThemes, setPrefs, usePrefs, type MessageDisplay } from "@/lib/prefs";
import { clickPoint, switchTheme } from "@/lib/theme-switch";
import { openSettings } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { PrefSetting, ThemeGrid } from "./common";

export function Appearance({ instanceKey }: { instanceKey?: string }) {
  const { t, number } = useI18n();
  const p = usePrefs((x) => x);
  return (
    <WithPreview preview={<ChatPreview instanceKey={instanceKey} />}>
      <div className="flex flex-col">
        <PrefSetting id="theme" title={t("appsettings.appearance.theme")} hint={t("appsettings.appearance.themeHint")} keys={["theme", "followSystem", "lightTheme", "darkTheme"]}>
          <Toggle
            checked={p.followSystem}
            onChange={(followSystem) => setPrefs({ followSystem })}
            label={t("appsettings.appearance.followSystem")}
            hint={t("appsettings.appearance.followSystemHint")}
          />
          <AnimatePresence mode="wait" initial={false}>
            {p.followSystem ? (
              <motion.div key="system" initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={SPRING} className="flex flex-col gap-4">
                <div>
                  <p className="mb-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("appsettings.appearance.whenLight")}</p>
                  <ThemeGrid id="light" themes={lightThemes(p)} value={p.lightTheme} onChange={(lightTheme, e) => switchTheme({ lightTheme }, clickPoint(e))} />
                </div>
                <div>
                  <p className="mb-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("appsettings.appearance.whenDark")}</p>
                  <ThemeGrid id="dark" themes={darkThemes(p)} value={p.darkTheme} onChange={(darkTheme, e) => switchTheme({ darkTheme }, clickPoint(e))} />
                </div>
              </motion.div>
            ) : (
              <motion.div key="fixed" initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={SPRING}>
                <ThemeGrid id="fixed" themes={allThemes(p)} value={p.theme} onChange={(theme, e) => switchTheme({ theme }, clickPoint(e))} onMake={() => openSettings("themes")} />
              </motion.div>
            )}
          </AnimatePresence>
        </PrefSetting>
        <PrefSetting id="density" title={t("appsettings.appearance.density")} hint={t("appsettings.appearance.densityHint")} keys={["density"]}>
          <Choice
            value={p.density}
            onChange={(density) => setPrefs({ density })}
            options={[
              { value: "compact", label: t("appsettings.appearance.compact"), hint: t("appsettings.appearance.compactHint"), icon: <Rows4Icon className="size-4" /> },
              { value: "default", label: t("appsettings.appearance.default"), hint: t("appsettings.appearance.defaultHint"), icon: <Rows3Icon className="size-4" /> },
              { value: "spacious", label: t("appsettings.appearance.spacious"), hint: t("appsettings.appearance.spaciousHint"), icon: <Rows2Icon className="size-4" /> },
            ]}
          />
        </PrefSetting>
        <PrefSetting id="message-display" title={t("appsettings.appearance.display")} keys={["messageDisplay"]}>
          <Choice<MessageDisplay>
            value={p.messageDisplay}
            onChange={(messageDisplay) => setPrefs({ messageDisplay })}
            options={[
              { value: "cozy", label: t("appsettings.appearance.cozy"), hint: t("appsettings.appearance.cozyHint"), icon: <MessageSquareTextIcon className="size-4" /> },
              { value: "compact", label: t("appsettings.appearance.compact"), hint: t("appsettings.appearance.compactDisplayHint"), icon: <AlignJustifyIcon className="size-4" /> },
            ]}
          />
        </PrefSetting>
        <PrefSetting id="chat-font-size" title={t("appsettings.appearance.textSize")} keys={["chatFontSize"]}>
          <Slider
            label={t("appsettings.appearance.textSize")}
            value={p.chatFontSize}
            min={12}
            max={20}
            format={(n) => `${number(n)}px`}
            onChange={(chatFontSize) => setPrefs({ chatFontSize })}
            marks={[
              { value: 12, label: `${number(12)}px` },
              { value: 15, label: `${number(15)}px` },
              { value: 20, label: `${number(20)}px` },
            ]}
          />
        </PrefSetting>
        <Zoom />
      </div>
    </WithPreview>
  );
}

/** The whole app scales under the pointer, so zoom applies when the slider is let go. */
function Zoom() {
  const { t, number } = useI18n();
  const percent = (n: number) => number(n / 100, { style: "percent" });
  const zoom = usePrefs((p) => p.zoom);
  const [value, setValue] = useState(zoom);
  useEffect(() => setValue(zoom), [zoom]);
  return (
    <PrefSetting id="zoom" title={t("appsettings.appearance.zoom")} hint={t("appsettings.appearance.zoomHint")} keys={["zoom"]}>
      <Slider
        label={t("appsettings.appearance.zoom")}
        value={value}
        min={80}
        max={150}
        step={10}
        format={percent}
        onChange={setValue}
        onCommit={(z) => setPrefs({ zoom: z })}
        marks={[
          { value: 80, label: percent(80) },
          { value: 100, label: percent(100) },
          { value: 150, label: percent(150) },
        ]}
      />
    </PrefSetting>
  );
}

const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000);

/** A few messages in the current look, so density, display and text size show before you leave. */
export function ChatPreview({ instanceKey }: { instanceKey?: string }) {
  const { t } = useI18n();
  const display = usePrefs((p) => p.messageDisplay);
  usePrefs((p) => p.clock);
  const me = useInstance(instanceKey)?.me;
  const hana = create(UserSchema, { id: "01HANA", username: "hana", displayName: "Hana" });
  const you = me ?? create(UserSchema, { id: "01YOU", username: "you", displayName: t("appsettings.preview.you") });
  const lines = [
    { author: hana, first: true, at: ago(6), text: t("appsettings.preview.newThemes") },
    { author: hana, first: false, at: ago(5), text: t("appsettings.preview.summer", { theme: "Sora" }) },
    { author: you, first: true, at: ago(1), text: t("appsettings.preview.favorite", { theme: "**Matcha**" }) },
  ];
  return (
    <div className="overflow-hidden rounded-3xl border bg-background py-3 shadow-lg">
      {lines.map((line, n) => (
        <motion.div
          key={n}
          layout="position"
          transition={SPRING}
          className={cn("message-row flex gap-3 px-4", line.first && "first", display === "compact" && "compact")}
        >
          <MessageLine display={display} first={line.first} author={line.author} member={undefined} date={line.at}>
            <MessageBody content={line.text} display={display} />
          </MessageLine>
        </motion.div>
      ))}
    </div>
  );
}

/** Motion follows the system unless someone picks otherwise. */
export function MotionChoice() {
  const { t } = useI18n();
  const value = usePrefs((p) => p.reduceMotion);
  return (
    <Choice
      value={value}
      onChange={(reduceMotion) => setPrefs({ reduceMotion })}
      options={[
        { value: "system", label: t("appsettings.accessibility.motionSystem"), hint: t("appsettings.accessibility.motionSystemHint"), icon: <MonitorIcon className="size-4" /> },
        { value: "always", label: t("appsettings.accessibility.motionReduce"), hint: t("appsettings.accessibility.motionReduceHint"), icon: <SnailIcon className="size-4" /> },
        { value: "never", label: t("appsettings.accessibility.motionFull"), hint: t("appsettings.accessibility.motionFullHint"), icon: <SparklesIcon className="size-4" /> },
      ]}
    />
  );
}
