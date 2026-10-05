import { ClockIcon, CornerDownLeftIcon, MonitorIcon, SendHorizontalIcon } from "lucide-react";
import { Choice } from "@/components/settings/controls";
import { useI18n } from "@/i18n/react";
import { comboLabel } from "@/lib/keybinds";
import { setPrefs, usePrefs, type Clock, type SendWith } from "@/lib/prefs";
import { PrefSetting } from "./common";

const sample = new Date(2026, 0, 1, 15, 4);

export function Chat() {
  const { t, date } = useI18n();
  // In the app's language, as message times are (lib/format.ts).
  const at = (hourCycle?: "h12" | "h23") => date(sample, { hour: "numeric", minute: "2-digit", hourCycle });
  const p = usePrefs((x) => x);
  return (
    <div className="flex flex-col">
      <PrefSetting id="clock" title={t("appsettings.chat.clock")} hint={t("appsettings.chat.clockHint")} keys={["clock"]}>
        <Choice<Clock>
          value={p.clock}
          onChange={(clock) => setPrefs({ clock })}
          options={[
            { value: "auto", label: t("appsettings.chat.clockAuto"), hint: at(), icon: <MonitorIcon className="size-4" /> },
            { value: "12h", label: t("appsettings.chat.clock12"), hint: at("h12"), icon: <ClockIcon className="size-4" /> },
            { value: "24h", label: t("appsettings.chat.clock24"), hint: at("h23"), icon: <ClockIcon className="size-4" /> },
          ]}
        />
      </PrefSetting>
      <PrefSetting id="send-with" title={t("appsettings.chat.sendWith")} hint={t("appsettings.chat.sendWithHint")} keys={["sendWith"]} delay={0.04}>
        <Choice<SendWith>
          value={p.sendWith}
          onChange={(sendWith) => setPrefs({ sendWith })}
          options={[
            { value: "enter", label: comboLabel("Enter"), hint: t("appsettings.chat.newLine", { keys: comboLabel("Shift+Enter") }), icon: <SendHorizontalIcon className="size-4" /> },
            {
              value: "mod-enter",
              label: comboLabel("Mod+Enter"),
              hint: t("appsettings.chat.newLineMarkdown", { keys: comboLabel("Enter") }),
              icon: <CornerDownLeftIcon className="size-4" />,
            },
          ]}
        />
      </PrefSetting>
    </div>
  );
}
