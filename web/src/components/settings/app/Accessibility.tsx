import { CircleIcon, CircleOffIcon, EyeIcon, GlassesIcon, PaletteIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { Markdown } from "@/components/Markdown";
import { RoleName } from "@/components/RoleName";
import { Choice, Toggle } from "@/components/settings/controls";
import { Slider } from "@/components/ui/slider";
import { useI18n } from "@/i18n/react";
import { SPRING } from "@/lib/motion";
import { reduceMotion, setPrefs, usePrefs } from "@/lib/prefs";
import { MotionChoice } from "./Appearance";
import { PrefSetting } from "./common";

/** Colors people with color blindness mix up: red, green, amber, blue, purple and pink. The page's filter corrects them like everything else. */
const SWATCHES = ["#e5484d", "#2fb47c", "#f5a524", "#3b8beb", "#b388ff", "#f06292"];

export function Accessibility() {
  const { t, number } = useI18n();
  const percent = (n: number) => number(n / 100, { style: "percent" });
  const p = usePrefs((x) => x);
  const systemCalm = typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  return (
    <div className="flex flex-col">
      <PrefSetting
        id="reduce-motion"
        title={t("appsettings.accessibility.motion")}
        hint={
          p.reduceMotion === "system"
            ? systemCalm
              ? t("appsettings.accessibility.motionSystemCalm")
              : t("appsettings.accessibility.motionSystemMoves")
            : reduceMotion(p)
              ? t("appsettings.accessibility.motionOff")
              : t("appsettings.accessibility.motionOn")
        }
        keys={["reduceMotion"]}
      >
        <MotionChoice />
      </PrefSetting>
      <PrefSetting id="saturation" title={t("appsettings.accessibility.saturation")} hint={t("appsettings.accessibility.saturationHint")} keys={["saturation"]} delay={0.04}>
        <Slider
          label={t("appsettings.accessibility.saturation")}
          value={p.saturation}
          min={0}
          max={100}
          step={5}
          format={percent}
          onChange={(saturation) => setPrefs({ saturation })}
          marks={[
            { value: 0, label: percent(0) },
            { value: 50, label: percent(50) },
            { value: 100, label: percent(100) },
          ]}
        />
      </PrefSetting>
      <PrefSetting
        id="color-vision"
        title={t("appsettings.accessibility.colorVision")}
        hint={t("appsettings.accessibility.colorVisionHint")}
        keys={["colorVision", "colorVisionStrength"]}
        delay={0.05}
      >
        <Choice
          value={p.colorVision}
          onChange={(colorVision) => setPrefs({ colorVision })}
          options={[
            { value: "off", label: t("appsettings.accessibility.colorVisionOff"), hint: t("appsettings.accessibility.colorVisionOffHint"), icon: <EyeIcon className="size-4" /> },
            { value: "protanopia", label: t("appsettings.accessibility.protanopia"), hint: t("appsettings.accessibility.protanopiaHint"), icon: <GlassesIcon className="size-4" /> },
            { value: "deuteranopia", label: t("appsettings.accessibility.deuteranopia"), hint: t("appsettings.accessibility.deuteranopiaHint"), icon: <GlassesIcon className="size-4" /> },
            { value: "tritanopia", label: t("appsettings.accessibility.tritanopia"), hint: t("appsettings.accessibility.tritanopiaHint"), icon: <GlassesIcon className="size-4" /> },
          ]}
        />
        <AnimatePresence initial={false}>
          {p.colorVision !== "off" && (
            <motion.div key="strength" initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING}>
              <Slider
                label={t("appsettings.accessibility.colorVisionStrength")}
                value={p.colorVisionStrength}
                min={0}
                max={100}
                step={5}
                format={percent}
                onChange={(colorVisionStrength) => setPrefs({ colorVisionStrength })}
              />
            </motion.div>
          )}
        </AnimatePresence>
        <div aria-hidden className="flex gap-2 rounded-xl bg-muted/50 px-3 py-2">
          {SWATCHES.map((color) => (
            <span key={color} className="h-6 flex-1 rounded-md" style={{ background: color }} />
          ))}
        </div>
      </PrefSetting>
      <PrefSetting id="high-contrast" title={t("appsettings.accessibility.contrast")} keys={["highContrast"]} delay={0.055}>
        <Toggle
          checked={p.highContrast}
          onChange={(highContrast) => setPrefs({ highContrast })}
          label={t("appsettings.accessibility.contrastToggle")}
          hint={t("appsettings.accessibility.contrastToggleHint")}
        />
      </PrefSetting>
      <PrefSetting
        id="role-colors"
        title={t("appsettings.accessibility.roleColors")}
        hint={t("appsettings.accessibility.roleColorsHint")}
        keys={["roleColors"]}
        delay={0.06}
      >
        <Choice
          value={p.roleColors}
          onChange={(roleColors) => setPrefs({ roleColors })}
          options={[
            { value: "names", label: t("appsettings.accessibility.roleNames"), hint: t("appsettings.accessibility.roleNamesHint"), icon: <PaletteIcon className="size-4" /> },
            { value: "beside", label: t("appsettings.accessibility.roleBeside"), hint: t("appsettings.accessibility.roleBesideHint"), icon: <CircleIcon className="size-4" /> },
            { value: "off", label: t("appsettings.accessibility.roleOff"), hint: t("appsettings.accessibility.roleOffHint"), icon: <CircleOffIcon className="size-4" /> },
          ]}
        />
        <div className="flex flex-wrap gap-x-5 gap-y-1 rounded-xl bg-muted/50 px-3 py-2 text-sm">
          <RoleName id="preview-sakura" name="Sakura" color={0xf472b6} />
          <RoleName id="preview-ren" name="Ren" color={0x60a5fa} />
          <RoleName id="preview-mio" name="Mio" color={0x34d399} />
        </div>
      </PrefSetting>
      <PrefSetting
        id="others-effects"
        title={t("appsettings.accessibility.effects")}
        hint={reduceMotion(p) ? t("appsettings.accessibility.effectsStill") : t("appsettings.accessibility.effectsHint")}
        keys={["othersEffects"]}
        delay={0.07}
      >
        <Toggle
          checked={p.othersEffects}
          onChange={(othersEffects) => setPrefs({ othersEffects })}
          label={t("appsettings.accessibility.effectsToggle")}
          hint={t("appsettings.accessibility.effectsToggleHint")}
        />
      </PrefSetting>
      <PrefSetting id="underline-links" title={t("appsettings.accessibility.links")} keys={["underlineLinks"]} delay={0.08}>
        <Toggle
          checked={p.underlineLinks}
          onChange={(underlineLinks) => setPrefs({ underlineLinks })}
          label={t("appsettings.accessibility.linksToggle")}
          hint={t("appsettings.accessibility.linksToggleHint")}
        />
        <div className="rounded-xl bg-muted/50 px-3 py-2 text-sm">
          <Markdown className="chat">{t("appsettings.accessibility.linksSample", { link: `[${t("appsettings.accessibility.linksSampleLink")}](https://github.com/waifu-devs/fuwa)` })}</Markdown>
        </div>
      </PrefSetting>
    </div>
  );
}
