import { ClockIcon, CornerDownLeftIcon, GlobeIcon, MonitorIcon, PlusIcon, SendHorizontalIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState } from "react";
import { Choice } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useI18n } from "@/i18n/react";
import { comboLabel } from "@/lib/keybinds";
import { cleanDomain } from "@/lib/links";
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
      <PrefSetting id="trusted-sites" title={t("appsettings.chat.trusted")} hint={t("appsettings.chat.trustedHint")} keys={["trustedDomains"]} delay={0.08}>
        <TrustedSites domains={p.trustedDomains} />
      </PrefSetting>
    </div>
  );
}

/** The sites whose masked links open without asking: added here or from the dialog that asks, removed here. */
function TrustedSites({ domains }: { domains: string[] }) {
  const { t } = useI18n();
  const [draft, setDraft] = useState("");
  const site = cleanDomain(draft);
  const add = () => {
    if (!site) return;
    setPrefs((x) => ({ trustedDomains: [...x.trustedDomains, site] }));
    setDraft("");
  };
  return (
    <div className="flex flex-col gap-2">
      <form
        className="flex gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          add();
        }}
      >
        <Input value={draft} onChange={(e) => setDraft(e.target.value)} placeholder={t("appsettings.chat.trustedPlaceholder")} aria-label={t("appsettings.chat.trustedPlaceholder")} />
        <Button type="submit" variant="outline" disabled={!site || domains.includes(site)}>
          <PlusIcon />
          {t("appsettings.chat.trustedAdd")}
        </Button>
      </form>
      {domains.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("appsettings.chat.trustedNone")}</p>
      ) : (
        <ul className="flex flex-col gap-1">
          <AnimatePresence initial={false}>
            {domains.map((domain) => (
              <motion.li
                key={domain}
                layout
                initial={{ opacity: 0, y: -4, scale: 0.98 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, scale: 0.96 }}
                transition={{ type: "spring", stiffness: 460, damping: 32 }}
                className="flex items-center gap-2 rounded-xl border px-3 py-1.5 text-sm"
              >
                <GlobeIcon className="size-4 text-muted-foreground" />
                <span className="min-w-0 flex-1 truncate font-mono">{domain}</span>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label={t("appsettings.chat.trustedRemove", { site: domain })}
                  title={t("appsettings.chat.trustedRemove", { site: domain })}
                  onClick={() => setPrefs((x) => ({ trustedDomains: x.trustedDomains.filter((d) => d !== domain) }))}
                >
                  <XIcon />
                </Button>
              </motion.li>
            ))}
          </AnimatePresence>
        </ul>
      )}
    </div>
  );
}
