import { PaletteIcon } from "lucide-react";
import { motion } from "motion/react";
import { SPRING } from "@/components/motion";
import { BackdropForm } from "@/components/settings/app/BackdropForm";
import { ThemePreview } from "@/components/settings/app/ThemePreview";
import { WithPreview } from "@/components/settings/controls";
import { T, useI18n } from "@/i18n/react";
import { activeBackdrop, activeTheme, setPrefs, usePrefs } from "@/lib/prefs";
import { openSettings } from "@/lib/ui";
import { PrefSetting } from "./common";

/** The picture and effect behind the app, under any theme that doesn't bring its own. */
export function Backgrounds({ instanceKey }: { instanceKey?: string }) {
  const { t } = useI18n();
  const theme = usePrefs(activeTheme);
  const backdrop = usePrefs((p) => p.backdrop);
  const shown = usePrefs(activeBackdrop);
  const own = "backdrop" in theme && !!theme.backdrop;
  return (
    <WithPreview preview={<ThemePreview theme={theme} backdrop={shown} />}>
      {own && (
        <motion.div initial={{ opacity: 0, y: -6 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="mb-6 flex items-center gap-3 rounded-2xl border border-primary/30 bg-primary/10 p-3 text-sm">
          <PaletteIcon className="size-4 shrink-0 text-primary" />
          <span className="flex-1">
            <T k="appsettings.backgrounds.themeHasOwn" values={{ theme: <b>{theme.name}</b> }} />
          </span>
          <button type="button" onClick={() => openSettings("themes")} className="text-xs font-bold text-primary hover:underline">
            {t("appsettings.backgrounds.editTheme")}
          </button>
        </motion.div>
      )}
      <PrefSetting id="backdrop" title={t("appsettings.backgrounds.title")} hint={t("appsettings.backgrounds.hint")} keys={["backdrop"]}>
        <BackdropForm instanceKey={instanceKey} value={backdrop} onChange={(patch) => setPrefs((p) => ({ backdrop: { ...p.backdrop, ...patch } }))} />
      </PrefSetting>
    </WithPreview>
  );
}
