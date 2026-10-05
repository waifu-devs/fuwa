import { clone } from "@bufbuild/protobuf";
import { CircleCheckIcon, CircleXIcon, FilmIcon, KeyRoundIcon, LoaderCircleIcon, PowerOffIcon, ShieldIcon, SparklesIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState } from "react";
import { GifSettingsSchema, type GifSettings as GifSetup, type InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import type { TestGifProviderResponse } from "@/gen/fuwa/v1/gif_pb";
import { GifProvider } from "@/gen/fuwa/v1/types_pb";
import { run, testGifProvider } from "@/fuwa/actions";
import { usePrivateField } from "@/components/Private";
import { GifImage } from "@/components/chat/GifImage";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { formatBytes } from "@/lib/format";
import { type I18n, useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";
import { Cap, Choice, Setting, SPRING } from "../controls";
import { gifsOf } from "./gif-fields";

type Reset = { changed: boolean; onReset: () => void; resetting: boolean };

const RATING_LABEL: Record<string, string> = { g: "G", pg: "PG", "pg-13": "PG-13", r: "R" };

/**
 * GIF search: which provider the instance asks and with what key, how
 * strict, and the caps. The provider never sees who searches: the instance
 * asks for everyone and hands every picture back itself.
 */
export function GifSettings({
  instanceKey,
  draft,
  saved,
  defaults,
  patch,
  resetter,
}: {
  instanceKey: string;
  draft: InstanceSettings;
  saved: InstanceSettings;
  defaults: InstanceSettings;
  patch: (fn: (d: InstanceSettings) => void) => void;
  resetter: (...paths: string[]) => Reset;
}) {
  const lang = useI18n();
  const { t } = lang;
  const g = gifsOf(draft);
  const was = gifsOf(saved);
  const def = gifsOf(defaults);
  const edit = (fn: (g: GifSetup) => void) =>
    patch((d) => {
      d.gifs = clone(GifSettingsSchema, gifsOf(d));
      fn(d.gifs);
    });
  const reset = resetter("gifs");

  return (
    <>
      <Setting
        id="gif-provider"
        title={t("instancesettings.nav.gifProvider")}
        hint={t("instancesettings.gifs.providerHint")}
        defaultLabel={providerLabel(t, def.provider)}
        {...reset}
      >
        <Choice
          value={g.provider}
          onChange={(provider) => edit((x) => (x.provider = provider))}
          options={[
            { value: GifProvider.UNSPECIFIED, label: t("serversettings.shared.off"), hint: t("instancesettings.gifs.offHint"), icon: <PowerOffIcon className="size-4" /> },
            { value: GifProvider.GIPHY, label: "GIPHY", hint: t("instancesettings.gifs.giphyHint"), icon: <SparklesIcon className="size-4" /> },
            { value: GifProvider.KLIPY, label: "Klipy", hint: t("instancesettings.gifs.klipyHint"), icon: <FilmIcon className="size-4" /> },
          ]}
        />
      </Setting>
      <AnimatePresence initial={false}>
        {g.provider !== GifProvider.UNSPECIFIED && (
          <motion.div key="on" initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 10 }} transition={SPRING}>
            <GifKeySetting instanceKey={instanceKey} g={g} was={was} def={def} reset={reset} edit={edit} />
            <Setting
              id="gif-rating"
              title={t("instancesettings.nav.gifRating")}
              hint={t("instancesettings.gifs.ratingHint")}
              delay={0.04}
              defaultLabel={RATING_LABEL[def.rating || "pg-13"]}
              {...reset}
            >
              <Choice
                value={g.rating || "pg-13"}
                onChange={(rating) => edit((x) => (x.rating = rating))}
                options={[
                  { value: "g", label: "G", hint: t("instancesettings.gifs.ratingG"), icon: <ShieldIcon className="size-4" /> },
                  { value: "pg", label: "PG", hint: t("instancesettings.gifs.ratingPg"), icon: <ShieldIcon className="size-4" /> },
                  { value: "pg-13", label: "PG-13", hint: t("instancesettings.gifs.ratingPg13"), icon: <ShieldIcon className="size-4" /> },
                  { value: "r", label: "R", hint: t("instancesettings.gifs.ratingR"), icon: <ShieldIcon className="size-4" /> },
                ]}
              />
            </Setting>
            <Setting
              id="gif-caps"
              title={t("instancesettings.gifs.caps")}
              hint={t("instancesettings.gifs.capsHint")}
              delay={0.06}
              defaultLabel={t("instancesettings.gifs.noCaps")}
              {...reset}
            >
              <div className="grid gap-3 sm:grid-cols-3">
                <Cap label={t("instancesettings.gifs.largest")} bytes value={g.gifBytes} onChange={(v) => edit((x) => (x.gifBytes = v))} />
                <Cap label={t("instancesettings.gifs.searches")} value={g.searchesPerMinute} onChange={(v) => edit((x) => (x.searchesPerMinute = v))} />
                <Cap label={t("instancesettings.gifs.calls")} value={g.providerCallsPerDay} onChange={(v) => edit((x) => (x.providerCallsPerDay = v))} />
              </div>
              {g.gifBytes !== undefined && (
                <p className="text-xs text-muted-foreground">
                  {t("instancesettings.gifs.bigger", { size: formatBytes(lang, Number(g.gifBytes)) })}
                </p>
              )}
            </Setting>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  );
}

/** The provider's API key: saved ones are never shown, only how they end. */
function GifKeySetting({
  instanceKey,
  g,
  was,
  def,
  reset,
  edit,
}: {
  instanceKey: string;
  g: GifSetup;
  was: GifSetup;
  def: GifSetup;
  reset: Reset;
  edit: (fn: (g: GifSetup) => void) => void;
}) {
  const { t } = useI18n();
  const privateField = usePrivateField();
  const name = g.provider === GifProvider.GIPHY ? "GIPHY" : g.provider === GifProvider.KLIPY ? "Klipy" : "";
  const keptKey = was.apiKeySet && was.provider === g.provider;
  const keyMissing = g.provider !== GifProvider.UNSPECIFIED && !g.apiKey.trim() && !keptKey;
  return (
    <Setting
      id="gif-key"
      title={t("instancesettings.gifs.keyTitle", { name })}
      hint={t("instancesettings.gifs.keyHint", { name })}
      delay={0.02}
      defaultLabel={t(def.apiKeySet ? "instancesettings.shared.set" : "instancesettings.shared.none")}
      {...reset}
    >
      <div className="relative">
        <KeyRoundIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          type="password"
          autoComplete="off"
          value={g.apiKey}
          onChange={(e) => edit((x) => (x.apiKey = e.target.value))}
          placeholder={
            keptKey
              ? was.apiKeyHint
                ? t("instancesettings.shared.savedEnding", { hint: was.apiKeyHint })
                : t("instancesettings.shared.saved")
              : t("instancesettings.gifs.paste", { name })
          }
          className={cn("h-10 rounded-xl pl-9 font-mono text-sm", privateField, keyMissing && "ring-2 ring-amber-500/50")}
        />
      </div>
      {keyMissing && <p className="text-xs text-amber-600 dark:text-amber-400">{t("instancesettings.gifs.keyMissing")}</p>}
      <TryIt instanceKey={instanceKey} settings={g} />
    </Setting>
  );
}

function providerLabel(t: I18n["t"], p: GifProvider) {
  return p === GifProvider.GIPHY ? "GIPHY" : p === GifProvider.KLIPY ? "Klipy" : t("instancesettings.shared.off");
}

/** Asks the provider for a few trending GIFs with what's typed, before it's saved. */
function TryIt({ instanceKey, settings }: { instanceKey: string; settings: GifSetup }) {
  const { t } = useI18n();
  const [state, setState] = useState<"idle" | "trying" | TestGifProviderResponse>("idle");
  async function go() {
    setState("trying");
    try {
      setState(await run(testGifProvider(instanceKey, settings)));
    } catch (err) {
      setState({ ok: false, error: (err as Error).message, elapsedMs: 0, results: [] } as unknown as TestGifProviderResponse);
    }
  }
  const result = typeof state === "object" ? state : null;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-3">
        <Button type="button" variant="outline" size="sm" onClick={go} disabled={state === "trying"} className="btn h-9 rounded-xl px-4 font-bold">
          {state === "trying" ? <LoaderCircleIcon className="size-4 animate-spin" /> : <SparklesIcon className="size-4" />}
          {t("instancesettings.gifs.tryIt")}
        </Button>
        <AnimatePresence mode="popLayout" initial={false}>
          {result && (
            <motion.span
              key={result.ok ? "ok" : "no"}
              initial={{ opacity: 0, x: -6 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0 }}
              transition={SPRING}
              className={cn("flex items-center gap-1.5 text-sm font-bold", result.ok ? "text-emerald-600 dark:text-emerald-400" : "text-destructive")}
            >
              {result.ok ? <CircleCheckIcon className="size-4" /> : <CircleXIcon className="size-4" />}
              {result.ok ? t("instancesettings.gifs.works", { ms: String(result.elapsedMs) }) : result.error}
            </motion.span>
          )}
        </AnimatePresence>
      </div>
      {result?.ok && result.results.length > 0 && (
        <div className="grid grid-cols-3 gap-1.5 sm:grid-cols-6">
          {result.results.map((r, n) => (
            <motion.span
              key={r.id}
              initial={{ opacity: 0, scale: 0.9 }}
              animate={{ opacity: 1, scale: 1 }}
              transition={{ ...SPRING, delay: n * 0.04 }}
              className="aspect-square overflow-hidden rounded-lg bg-muted"
            >
              <GifImage src={r.previewUrl} still={r.stillUrl} playing alt={r.title} className="size-full object-cover" />
            </motion.span>
          ))}
        </div>
      )}
    </div>
  );
}
