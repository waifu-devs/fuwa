import { ClockIcon, CornerDownLeftIcon, MonitorIcon, SendHorizontalIcon } from "lucide-react";
import { Choice } from "@/components/settings/controls";
import { comboLabel } from "@/lib/keybinds";
import { setPrefs, usePrefs, type Clock, type SendWith } from "@/lib/prefs";
import { PrefSetting } from "./common";

const sample = new Date(2026, 0, 1, 15, 4);
const at = (hourCycle?: "h12" | "h23") => new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", hourCycle }).format(sample);

export function Chat() {
  const p = usePrefs((x) => x);
  return (
    <div className="flex flex-col">
      <PrefSetting id="clock" title="Time format" hint="For message times everywhere." keys={["clock"]}>
        <Choice<Clock>
          value={p.clock}
          onChange={(clock) => setPrefs({ clock })}
          options={[
            { value: "auto", label: "My language's", hint: at(), icon: <MonitorIcon className="size-4" /> },
            { value: "12h", label: "12 hour", hint: at("h12"), icon: <ClockIcon className="size-4" /> },
            { value: "24h", label: "24 hour", hint: at("h23"), icon: <ClockIcon className="size-4" /> },
          ]}
        />
      </PrefSetting>
      <PrefSetting id="send-with" title="Send messages with" hint="The other one starts a new line." keys={["sendWith"]} delay={0.04}>
        <Choice<SendWith>
          value={p.sendWith}
          onChange={(sendWith) => setPrefs({ sendWith })}
          options={[
            { value: "enter", label: comboLabel("Enter"), hint: `${comboLabel("Shift+Enter")} for a new line.`, icon: <SendHorizontalIcon className="size-4" /> },
            {
              value: "mod-enter",
              label: comboLabel("Mod+Enter"),
              hint: `${comboLabel("Enter")} for a new line, handy for long Markdown.`,
              icon: <CornerDownLeftIcon className="size-4" />,
            },
          ]}
        />
      </PrefSetting>
    </div>
  );
}
