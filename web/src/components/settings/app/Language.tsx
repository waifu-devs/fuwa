import { LanguagesIcon, MonitorIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState } from "react";
import { SPRING } from "@/lib/motion";
import { Choice } from "@/components/settings/controls";
import { coverage, LANGUAGES, languageOf, loadCatalog } from "@/i18n/catalogs";
import { browserLanguage, switchLanguage } from "@/i18n/i18n";
import { useI18n } from "@/i18n/react";
import { setPrefs, usePrefs } from "@/lib/prefs";
import { PrefSetting } from "./common";

/** Each shipped language's share of English, once the page is open (small same-origin files). */
function useCoverage() {
  const [shares, setShares] = useState<Record<string, number>>({});
  useEffect(() => {
    let live = true;
    for (const { code } of LANGUAGES) {
      if (code === "en") continue;
      loadCatalog(code)
        .then((catalog) => live && setShares((s) => ({ ...s, [code]: coverage(catalog) })))
        .catch(() => {});
    }
    return () => void (live = false);
  }, []);
  return shares;
}

export function Language() {
  const { t, number } = useI18n();
  const language = usePrefs((p) => p.language);
  const shares = useCoverage();
  const [failed, setFailed] = useState(false);
  const pick = async (code: string) => {
    setFailed(false);
    // The strings come first, so the app never shows the new language half-loaded; then the setting is kept.
    if (!(await switchLanguage(code))) return setFailed(true);
    setPrefs({ language: code });
  };
  const note = (code: string) => {
    const l = languageOf(code);
    const share = shares[code];
    return [l.english !== l.name ? l.english : null, share !== undefined && share < 1 ? t("settings.language.coverage", { percent: number(share, { style: "percent" }) }) : null, l.reviewed ? null : t("settings.language.draft")]
      .filter(Boolean)
      .join(" · ");
  };
  return (
    <div className="flex flex-col">
      <PrefSetting id="language" title={t("settings.language.title")} keys={["language"]}>
        <Choice<string>
          value={LANGUAGES.some((l) => l.code === language) ? language : "auto"}
          onChange={pick}
          options={[
            {
              value: "auto",
              label: t("settings.language.matchBrowser"),
              hint: t("settings.language.matchBrowserNote", { language: languageOf(browserLanguage()).name }),
              icon: <MonitorIcon className="size-4" />,
            },
            ...LANGUAGES.map((l) => ({ value: l.code, label: l.name, hint: note(l.code), icon: <LanguagesIcon className="size-4" /> })),
          ]}
        />
        <AnimatePresence initial={false}>
          {failed && (
            <motion.p role="alert" initial={{ opacity: 0, y: -4 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} transition={SPRING} className="mt-2 text-xs text-destructive">
              {t("settings.language.failed")}
            </motion.p>
          )}
        </AnimatePresence>
      </PrefSetting>
    </div>
  );
}
