import { CodeIcon, FileUpIcon, LaptopIcon, LoaderCircleIcon, MonitorIcon, SearchIcon, TypeIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Choice, type ChoiceOption } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useI18n } from "@/i18n/react";
import { canListFonts, hasFont, importFont, listFonts, loadFontFile } from "@/lib/font-files";
import { cleanFamily, FONT_EXTENSIONS, FONT_TYPES, fontStack, MOST_FONT_BYTES, quoteFamily, searchFamilies, type FontPick, type FontRole } from "@/lib/fonts";
import { SPRING } from "@/lib/motion";
import { setPrefs, usePrefs } from "@/lib/prefs";
import { cn } from "@/lib/utils";
import { PrefSetting } from "./common";

type Kind = FontPick["kind"];

const KEY: Record<FontRole, "font" | "codeFont"> = { app: "font", code: "codeFont" };

/** The app's font, or the one for code: the bundled one, the system's, one on this computer, or a file. */
export function FontSetting({ role }: { role: FontRole }) {
  const { t } = useI18n();
  const key = KEY[role];
  const pick = usePrefs((p) => p[key]);
  // Installed and file need a font before they're saved, so the choice runs ahead of the setting.
  const [kind, setKind] = useState<Kind>(pick.kind);
  useEffect(() => setKind(pick.kind), [pick]);
  const save = (next: FontPick) => setPrefs({ [key]: next });

  const options: ChoiceOption<Kind>[] = [
    role === "app"
      ? { value: "default", label: t("appsettings.appearance.fontDefault"), hint: t("appsettings.appearance.fontDefaultHint"), icon: <TypeIcon className="size-4" /> }
      : { value: "default", label: t("appsettings.appearance.codeFontDefault"), hint: t("appsettings.appearance.codeFontDefaultHint"), icon: <CodeIcon className="size-4" /> },
    ...(role === "app"
      ? [{ value: "system" as const, label: t("appsettings.appearance.fontSystem"), hint: t("appsettings.appearance.fontSystemHint"), icon: <MonitorIcon className="size-4" /> }]
      : []),
    { value: "installed", label: t("appsettings.appearance.fontInstalled"), hint: t("appsettings.appearance.fontInstalledHint"), icon: <LaptopIcon className="size-4" /> },
    { value: "file", label: t("appsettings.appearance.fontFile"), hint: t("appsettings.appearance.fontFileHint"), icon: <FileUpIcon className="size-4" /> },
  ];

  return (
    <PrefSetting
      id={role === "app" ? "font" : "code-font"}
      title={t(role === "app" ? "appsettings.appearance.font" : "appsettings.appearance.codeFont")}
      hint={t(role === "app" ? "appsettings.appearance.fontHint" : "appsettings.appearance.codeFontHint")}
      keys={[key]}
    >
      <Choice<Kind>
        value={kind}
        options={options}
        onChange={(next) => {
          setKind(next);
          if (next === "default" || next === "system") save({ kind: next });
        }}
      />
      <AnimatePresence mode="wait" initial={false}>
        {kind === "installed" && (
          <Panel key="installed">
            <Installed family={pick.kind === "installed" ? pick.family : ""} onPick={(family) => save({ kind: "installed", family })} />
          </Panel>
        )}
        {kind === "file" && (
          <Panel key="file">
            <FromFile pick={pick.kind === "file" ? pick : undefined} onPick={save} />
          </Panel>
        )}
      </AnimatePresence>
      <Sample pick={pick} role={role} />
    </PrefSetting>
  );
}

function Panel({ children }: { children: ReactNode }) {
  return (
    <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={SPRING} className="flex flex-col gap-2">
      {children}
    </motion.div>
  );
}

/** A line in the picked font, so it shows before you leave. */
function Sample({ pick, role }: { pick: FontPick; role: FontRole }) {
  const { t } = useI18n();
  const missing = pick.kind === "installed" && !hasFont(pick.family);
  return (
    <div className="flex flex-col gap-1">
      <motion.p
        key={JSON.stringify(pick)}
        initial={{ opacity: 0, y: 4 }}
        animate={{ opacity: 1, y: 0 }}
        transition={SPRING}
        style={{ fontFamily: fontStack(pick, role) }}
        className="truncate rounded-xl bg-muted/60 px-3 py-2 text-base"
      >
        {t("appsettings.appearance.fontSample")}
      </motion.p>
      {missing && <p className="text-xs font-bold text-amber-600 dark:text-amber-400">{t("appsettings.appearance.fontMissing", { name: pick.family })}</p>}
    </div>
  );
}

/** A font on this computer: from the browser's list where it can give one, or by name. */
function Installed({ family, onPick }: { family: string; onPick: (family: string) => void }) {
  const { t } = useI18n();
  const [query, setQuery] = useState(family);
  const [fonts, setFonts] = useState<string[] | null>(null);
  const [listing, setListing] = useState(false);
  useEffect(() => setQuery(family), [family]);
  const shown = useMemo(() => (fonts ? searchFamilies(fonts, query === family ? "" : query) : []), [fonts, query, family]);
  const commit = () => {
    const name = cleanFamily(query);
    if (name && name !== family) onPick(name);
  };
  return (
    <>
      <div className="relative">
        <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onBlur={() => !fonts && commit()}
          onKeyDown={(e) => e.key === "Enter" && commit()}
          placeholder={fonts ? t("appsettings.appearance.fontSearch") : t("appsettings.appearance.fontName")}
          aria-label={t("appsettings.appearance.fontName")}
          className="rounded-xl pl-9"
          spellCheck={false}
          autoComplete="off"
        />
      </div>
      {canListFonts() && !fonts && (
        <Button
          variant="outline"
          size="sm"
          className="self-start rounded-xl"
          disabled={listing}
          onClick={async () => {
            setListing(true);
            setFonts(await listFonts());
            setListing(false);
          }}
        >
          {listing ? <LoaderCircleIcon className="animate-spin" /> : <LaptopIcon />}
          {t("appsettings.appearance.fontListFonts")}
        </Button>
      )}
      {fonts && (
        <div className="max-h-64 overflow-y-auto rounded-xl border p-1">
          {shown.length === 0 ? (
            <p className="px-3 py-2 text-sm text-muted-foreground">{t("appsettings.appearance.fontNone")}</p>
          ) : (
            shown.map((name) => (
              <motion.button
                key={name}
                type="button"
                whileTap={{ scale: 0.98 }}
                onClick={() => onPick(name)}
                className={cn(
                  "relative block w-full truncate rounded-lg px-3 py-1.5 text-left text-sm transition-colors hover:bg-muted",
                  name === family && "text-primary",
                )}
                style={{ fontFamily: `${quoteFamily(name)}, inherit` }}
              >
                {name === family && <motion.span layoutId="font-picked" transition={SPRING} className="absolute inset-0 rounded-lg bg-primary/10" />}
                <span className="relative">{name}</span>
              </motion.button>
            ))
          )}
        </div>
      )}
    </>
  );
}

/** A font file kept on this device. */
function FromFile({ pick, onPick }: { pick: Extract<FontPick, { kind: "file" }> | undefined; onPick: (pick: FontPick) => void }) {
  const { t, number } = useI18n();
  const input = useRef<HTMLInputElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const size = `${number(MOST_FONT_BYTES / 1024 / 1024)} MB`;
  useEffect(() => {
    if (pick) void loadFontFile(pick.id);
  }, [pick]);
  const add = async (file: File | undefined) => {
    if (!file) return;
    setError(null);
    setBusy(true);
    const result = await importFont(file);
    setBusy(false);
    if ("error" in result) {
      setError(
        result.error === "tooBig"
          ? t("appsettings.appearance.fontTooBig", { size })
          : result.error === "notFont"
            ? t("appsettings.appearance.fontNotFont", { types: FONT_TYPES })
            : t("appsettings.appearance.fontCantSave"),
      );
      return;
    }
    onPick(result);
  };
  return (
    <>
      <div className="flex flex-wrap items-center gap-3">
        <Button variant="outline" size="sm" className="rounded-xl" disabled={busy} onClick={() => input.current?.click()}>
          {busy ? <LoaderCircleIcon className="animate-spin" /> : <FileUpIcon />}
          {pick ? t("appsettings.appearance.fontOtherFile") : t("appsettings.appearance.fontPickFile")}
        </Button>
        <AnimatePresence mode="popLayout" initial={false}>
          {pick && (
            <motion.span key={pick.id} initial={{ opacity: 0, x: -6 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0 }} transition={SPRING} className="min-w-0 truncate text-sm font-bold">
              {pick.name}
            </motion.span>
          )}
        </AnimatePresence>
      </div>
      <p className="text-xs text-muted-foreground">{t("appsettings.appearance.fontFileTypes", { types: FONT_TYPES, size })}</p>
      <AnimatePresence>
        {error && (
          <motion.p initial={{ opacity: 0, y: -4 }} animate={{ opacity: 1, y: 0, x: [0, -6, 5, -3, 0] }} exit={{ opacity: 0 }} className="text-xs font-bold text-destructive">
            {error}
          </motion.p>
        )}
      </AnimatePresence>
      <input ref={input} type="file" accept={FONT_EXTENSIONS.join(",")} hidden onChange={(e) => (void add(e.target.files?.[0]), (e.target.value = ""))} />
    </>
  );
}
