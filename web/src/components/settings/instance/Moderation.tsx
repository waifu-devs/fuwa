import { clone, create } from "@bufbuild/protobuf";
import { CheckIcon, FlaskConicalIcon, GlobeLockIcon, KeyRoundIcon, LoaderCircleIcon, PlusIcon, SparklesIcon, Trash2Icon, TriangleAlertIcon, WebhookIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState } from "react";
import {
  AutoModProviderSettingsSchema,
  type AutoModProviderSettings,
  type InstanceSettings,
  type TestAutoModProviderResponse,
} from "@/gen/fuwa/v1/admin_pb";
import { run, testAutoModProvider } from "@/fuwa/actions";
import { usePrivateField } from "@/components/Private";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { type I18n, type Key, T, useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";
import { Cap, Setting, SPRING } from "../controls";

type Reset = { changed: boolean; onReset: () => void; resetting: boolean };

/** A provider as its card shows it. */
type Shown = { name: string; host: string; blurb: string; models: { id: string; label: string; hint: string }[]; keyHelp: string; tint: string };

/** What fuwa knows about each provider, beyond what the instance sends. Names stay as they are; blurbs, hints and key help are catalog keys. */
const KNOWN: Record<string, { name: string; host: string; blurb: Key; models: { id: string; label: string; hint: Key }[]; keyHelp: Key; tint: string }> = {
  "typesafe-jev": {
    name: "TypeSafe Jev",
    host: "api.typesafe.ai",
    blurb: "instancesettings.moderation.jevBlurb",
    models: [
      { id: "jev-latest", label: "Jev", hint: "instancesettings.moderation.jevLatest" },
      { id: "jev-preview", label: "Jev preview", hint: "instancesettings.moderation.jevPreview" },
    ],
    keyHelp: "instancesettings.moderation.jevKeyHelp",
    tint: "from-sky-500/25 to-indigo-500/10 text-sky-600 dark:text-sky-300",
  },
  "cloudflare-clef": {
    name: "Cloudflare Clef",
    host: "api.cloudflare.com",
    blurb: "instancesettings.moderation.clefBlurb",
    models: [
      { id: "@cf/cloudflare/clef", label: "Clef", hint: "instancesettings.moderation.clefModel" },
      { id: "@cf/cloudflare/clef-flash", label: "Clef flash", hint: "instancesettings.moderation.clefFlash" },
    ],
    keyHelp: "instancesettings.moderation.clefKeyHelp",
    tint: "from-orange-500/25 to-amber-500/10 text-orange-600 dark:text-orange-300",
  },
};

/** A provider fuwa knows, in the app's language; one it doesn't goes by its id. */
function knownProvider(t: I18n["t"], id: string): Shown {
  const k = Object.hasOwn(KNOWN, id) ? KNOWN[id] : undefined;
  if (!k) return { name: id, host: "", blurb: "", models: [], keyHelp: "", tint: "" };
  return { ...k, blurb: t(k.blurb), keyHelp: t(k.keyHelp), models: k.models.map((m) => ({ ...m, hint: t(m.hint) })) };
}

/** What a provider of the admins' own still needs before it can be tested, as one sentence. */
const MISSING: Record<string, Key> = {
  name: "instancesettings.moderation.addName",
  address: "instancesettings.moderation.addAddress",
  key: "instancesettings.moderation.addKey",
  "name address": "instancesettings.moderation.addNameAddress",
  "name key": "instancesettings.moderation.addNameKey",
  "address key": "instancesettings.moderation.addAddressKey",
  "name address key": "instancesettings.moderation.addAll",
};

/** The admins' own providers ("custom" until the instance gives them an id). */
const isCustom = (p: AutoModProviderSettings) => p.id === "custom" || p.id.startsWith("custom-");
/** At most this many of their own (the instance's limit). */
const MAX_CUSTOM = 8;

/** Where an address points, or "" while it isn't an https URL yet. */
function hostOf(url: string) {
  try {
    const parsed = new URL(url.trim());
    return parsed.protocol === "https:" ? parsed.hostname : "";
  } catch {
    return "";
  }
}

/**
 * Moderation services, set up once for the whole instance: a key and a
 * switch each. Servers then pick one for their AutoMod's smart filter.
 */
export function ModerationSettings({
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
  /** What the instance does unless changed, from its environment. */
  defaults?: InstanceSettings;
  patch: (fn: (d: InstanceSettings) => void) => void;
  resetter: (...paths: string[]) => Reset;
}) {
  const { t } = useI18n();
  const reset = resetter("automod_providers");
  const customs = draft.automodProviders.filter(isCustom).length;
  const change = (n: number) => (fn: (p: AutoModProviderSettings) => void) =>
    patch((d) => {
      if (!d.automodProviders[n]) return;
      const next = clone(AutoModProviderSettingsSchema, d.automodProviders[n]);
      fn(next);
      d.automodProviders[n] = next;
    });
  return (
    <>
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={SPRING}
        className="mb-4 flex items-start gap-3 rounded-2xl border border-violet-500/30 bg-violet-500/5 p-3 text-sm"
      >
        <motion.span
          animate={{ rotate: [0, 12, -6, 0], scale: [1, 1.1, 1] }}
          transition={{ duration: 1.4, repeat: Infinity, repeatDelay: 5 }}
          className="grid size-8 shrink-0 place-items-center rounded-xl bg-violet-500/15 text-violet-600 dark:text-violet-300"
        >
          <SparklesIcon className="size-4" />
        </motion.span>
        <p className="text-muted-foreground">{t("instancesettings.moderation.intro")}</p>
      </motion.div>
      <Setting
        id="automod-checks-per-day"
        title={t("instancesettings.nav.automodChecks")}
        hint={t("instancesettings.moderation.checksHint")}
        defaultLabel={
          defaults?.automodChecksPerDay === undefined
            ? t("instancesettings.shared.noLimit")
            : t("instancesettings.shared.perDay", { count: Number(defaults.automodChecksPerDay) })
        }
        {...resetter("automod_checks_per_day")}
      >
        <Cap label={t("instancesettings.shared.upTo")} placeholder="1000" value={draft.automodChecksPerDay} onChange={(v) => patch((d) => (d.automodChecksPerDay = v))} />
      </Setting>
      <AnimatePresence initial={false}>
        {draft.automodProviders.map((provider, n) => (
          <motion.div
            // New ones share "custom" until saved, so they go by place.
            key={provider.id === "custom" ? `new-${n}` : provider.id}
            layout="position"
            exit={{ opacity: 0, scale: 0.96, transition: { duration: 0.18 } }}
            transition={SPRING}
          >
            <ProviderCard
              instanceKey={instanceKey}
              provider={provider}
              saved={saved.automodProviders.find((p) => p.id === provider.id)}
              delay={isCustom(provider) && provider.id === "custom" ? 0 : 0.04 + n * 0.05}
              reset={n === 0 ? reset : undefined}
              onChange={change(n)}
              onRemove={isCustom(provider) ? () => patch((d) => void d.automodProviders.splice(n, 1)) : undefined}
            />
          </motion.div>
        ))}
      </AnimatePresence>
      <motion.div layout="position" transition={SPRING} data-setting="automod-custom" className="pt-2">
        <motion.button
          type="button"
          whileHover={{ y: -2 }}
          whileTap={{ scale: 0.98 }}
          transition={SPRING}
          disabled={customs >= MAX_CUSTOM}
          onClick={() => patch((d) => void d.automodProviders.push(create(AutoModProviderSettingsSchema, { id: "custom" })))}
          className="group flex w-full items-center gap-3 rounded-3xl border-2 border-dashed p-4 text-left transition-colors hover:border-primary/50 hover:bg-primary/[0.03] disabled:pointer-events-none disabled:opacity-50"
        >
          <span className="grid size-10 shrink-0 place-items-center rounded-2xl bg-violet-500/15 text-violet-600 transition-transform group-hover:rotate-90 dark:text-violet-300">
            <PlusIcon className="size-5" />
          </span>
          <span className="min-w-0 flex-1">
            <span className="block font-extrabold">{t("instancesettings.moderation.addOwn")}</span>
            <span className="block text-xs text-muted-foreground">
              {customs >= MAX_CUSTOM ? t("instancesettings.moderation.max", { count: MAX_CUSTOM }) : t("instancesettings.moderation.addOwnHint")}
            </span>
          </span>
        </motion.button>
      </motion.div>
    </>
  );
}

type TestResult = TestAutoModProviderResponse | { ok: false; error: string; elapsedMs: number; scores: [] };

/** Trying a provider with a sample message, before it's saved. */
function useProviderTest(instanceKey: string, provider: AutoModProviderSettings) {
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<TestResult | null>(null);

  async function test() {
    setTesting(true);
    setResult(null);
    try {
      const sample = "Free nitro for everyone who logs in at discord-gift.example with their password!";
      setResult(await run(testAutoModProvider(instanceKey, create(AutoModProviderSettingsSchema, { ...provider }), sample)));
    } catch (e) {
      setResult({ ok: false, error: (e as Error).message, elapsedMs: 0, scores: [] });
    } finally {
      setTesting(false);
    }
  }

  return { testing, result, test };
}

/** How a provider's card shows it: one of the admins' own by what they typed, a known one by its catalog entry. */
function shownProvider(t: I18n["t"], provider: AutoModProviderSettings, host: string): Shown {
  if (!isCustom(provider)) return knownProvider(t, provider.id);
  return {
    name: provider.name.trim() || t("instancesettings.moderation.yourProvider"),
    host: host || t("instancesettings.moderation.anAddress"),
    blurb: t("instancesettings.moderation.customBlurb"),
    models: [],
    keyHelp: t("instancesettings.moderation.customKeyHelp"),
    tint: "from-violet-500/25 to-fuchsia-500/10 text-violet-600 dark:text-violet-300",
  };
}

/** What a provider still needs before it can be tested or turned on. */
function missingKey(provider: AutoModProviderSettings, host: string, hasKey: boolean): Key {
  if (isCustom(provider)) {
    return MISSING[[provider.name.trim() ? "" : "name", host ? "" : "address", provider.header.trim() && !hasKey ? "key" : ""].filter(Boolean).join(" ")] ?? "instancesettings.moderation.addAll";
  }
  return provider.id === "cloudflare-clef" ? "instancesettings.moderation.addTokenAccount" : "instancesettings.moderation.addKey";
}

/** Whether a provider has what it needs to be tested or turned on, and what it lacks. */
function readiness(provider: AutoModProviderSettings, saved: AutoModProviderSettings | undefined) {
  const custom = isCustom(provider);
  const host = custom ? hostOf(provider.url) : "";
  // A saved key stays with its address: a new address needs it typed again.
  const moved = custom && !!saved && saved.url.trim() !== provider.url.trim();
  const hasKey = (provider.apiKeySet && !moved) || provider.apiKey.trim() !== "";
  const needsAccount = provider.id === "cloudflare-clef";
  const ready = custom
    ? provider.name.trim() !== "" && host !== "" && (provider.header.trim() === "" || hasKey)
    : hasKey && (!needsAccount || provider.accountId.trim().length === 32);
  return { custom, host, moved, needsAccount, ready, missing: missingKey(provider, host, hasKey) };
}

function ProviderCard({
  instanceKey,
  provider,
  saved,
  delay,
  reset,
  onChange,
  onRemove,
}: {
  instanceKey: string;
  provider: AutoModProviderSettings;
  saved?: AutoModProviderSettings;
  delay: number;
  reset?: Reset;
  onChange: (fn: (p: AutoModProviderSettings) => void) => void;
  onRemove?: () => void;
}) {
  const { t } = useI18n();
  const { custom, host, moved, needsAccount, ready, missing } = readiness(provider, saved);
  const known = shownProvider(t, provider, host);

  return (
    <Setting
      id={`automod-${provider.id}`}
      title={known.name}
      delay={delay}
      badge={!!reset}
      defaultLabel={t("instancesettings.shared.off")}
      changed={reset?.changed}
      onReset={reset?.onReset}
      resetting={reset?.resetting}
    >
      <div className={cn("overflow-hidden rounded-3xl border transition-colors", provider.enabled ? "border-primary/40 bg-primary/[0.03]" : "", custom && !provider.enabled && "border-dashed")}>
        <ProviderSwitch provider={provider} known={known} host={host} live={saved?.enabled ?? false} ready={ready} onChange={onChange} />
        <div className="flex flex-col gap-3 border-t p-4">
          <div className="flex items-start gap-2.5 rounded-2xl bg-muted/60 p-3 text-xs text-muted-foreground">
            <GlobeLockIcon className="mt-0.5 size-4 shrink-0 text-primary" />
            <span>
              <T k="instancesettings.moderation.goesTo" values={{ host: <b className="break-all text-foreground">{known.host}</b> }} />
            </span>
          </div>
          {custom && <CustomFields provider={provider} host={host} onChange={onChange} />}
          <KeyField provider={provider} known={known} moved={moved} onChange={onChange} />
          {needsAccount && <AccountField provider={provider} onChange={onChange} />}
          {known.models.length > 1 && <ModelChoice provider={provider} known={known} onChange={onChange} />}
          <TestBox instanceKey={instanceKey} provider={provider} ready={ready} missing={missing} />
          {onRemove && (
            <Button type="button" variant="ghost" size="sm" className="self-start rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive" onClick={onRemove}>
              <Trash2Icon /> {provider.name.trim() ? t("instancesettings.moderation.remove", { name: provider.name.trim() }) : t("instancesettings.moderation.removeThis")}
            </Button>
          )}
        </div>
      </div>
    </Setting>
  );
}

/** The card's head: the provider, where it is, whether it's live, and its switch. */
function ProviderSwitch({
  provider,
  known,
  host,
  live,
  ready,
  onChange,
}: {
  provider: AutoModProviderSettings;
  known: Shown;
  host: string;
  live: boolean;
  ready: boolean;
  onChange: (fn: (p: AutoModProviderSettings) => void) => void;
}) {
  const { t } = useI18n();
  const custom = isCustom(provider);
  return (
    <label className="flex cursor-pointer items-center gap-3 p-3 pl-4">
      <motion.span
        animate={provider.enabled ? { scale: [1, 1.18, 1], rotate: [0, -8, 0] } : { scale: 1 }}
        transition={{ duration: 0.4 }}
        className={cn("grid size-10 shrink-0 place-items-center rounded-2xl bg-gradient-to-br text-sm font-extrabold", known.tint)}
      >
        {custom ? (
          <WebhookIcon className="size-5" />
        ) : (
          known.name
            .split(" ")
            .map((w) => w[0])
            .join("")
        )}
      </motion.span>
      <span className="min-w-0 flex-1">
        <span className="flex flex-wrap items-center gap-2">
          <span className={cn("font-mono text-xs font-bold text-muted-foreground", custom && !host && "font-sans font-normal italic")}>{known.host}</span>
          <AnimatePresence initial={false}>
            {live && (
              <motion.span
                initial={{ scale: 0 }}
                animate={{ scale: 1 }}
                exit={{ scale: 0 }}
                transition={SPRING}
                className="rounded-full bg-emerald-500/15 px-2 py-0.5 text-[0.65rem] font-bold text-emerald-600 uppercase dark:text-emerald-400"
              >
                {t("instancesettings.moderation.live")}
              </motion.span>
            )}
          </AnimatePresence>
        </span>
        <span className="block text-xs text-muted-foreground">{known.blurb}</span>
      </span>
      <Switch
        checked={provider.enabled}
        disabled={!provider.enabled && !ready}
        onCheckedChange={(on) => onChange((p) => (p.enabled = on))}
        aria-label={t(provider.enabled ? "instancesettings.moderation.turnOff" : "instancesettings.moderation.turnOn", { name: known.name })}
      />
    </label>
  );
}

/** One of the admins' own providers: its name, address, key header and model. */
function CustomFields({ provider, host, onChange }: { provider: AutoModProviderSettings; host: string; onChange: (fn: (p: AutoModProviderSettings) => void) => void }) {
  const { t } = useI18n();
  const privateField = usePrivateField();
  return (
    <>
      <div className="grid gap-3 sm:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
        <label className="flex flex-col gap-1.5">
          <span className="text-sm font-extrabold">{t("instancesettings.nav.name")}</span>
          <Input
            value={provider.name}
            maxLength={40}
            onChange={(e) => onChange((p) => (p.name = e.target.value))}
            placeholder={t("instancesettings.moderation.namePlaceholder")}
            className="h-10 rounded-xl"
          />
        </label>
        <label className="flex flex-col gap-1.5">
          <span className="text-sm font-extrabold">{t("instancesettings.moderation.address")}</span>
          <Input
            value={provider.url}
            type="url"
            inputMode="url"
            spellCheck={false}
            maxLength={512}
            onChange={(e) => onChange((p) => (p.url = e.target.value))}
            placeholder="https://moderation.example.com/v1/check"
            aria-invalid={provider.url.trim() !== "" && !host}
            className={cn("h-10 rounded-xl font-mono text-sm", privateField)}
          />
        </label>
      </div>
      <AnimatePresence initial={false}>
        {provider.url.trim() !== "" && !host && (
          <motion.span
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            transition={SPRING}
            className="-mt-1 text-xs text-amber-700 dark:text-amber-400"
          >
            {t("instancesettings.moderation.httpsOnly")}
          </motion.span>
        )}
      </AnimatePresence>
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="flex flex-col gap-1.5">
          <span className="text-sm font-extrabold">{t("instancesettings.moderation.keyHeader")}</span>
          <Input
            value={provider.header}
            spellCheck={false}
            maxLength={64}
            onChange={(e) => onChange((p) => (p.header = e.target.value.trim()))}
            placeholder="Authorization (Bearer)"
            className="h-10 rounded-xl font-mono text-sm"
          />
        </label>
        <label className="flex flex-col gap-1.5">
          <span className="text-sm font-extrabold">{t("instancesettings.moderation.model")}</span>
          <Input
            value={provider.model}
            spellCheck={false}
            maxLength={100}
            onChange={(e) => onChange((p) => (p.model = e.target.value))}
            placeholder={t("instancesettings.moderation.modelOptional")}
            className="h-10 rounded-xl font-mono text-sm"
          />
        </label>
      </div>
    </>
  );
}

/** The provider's key (a token for Cloudflare), never shown again once saved. */
function KeyField({
  provider,
  known,
  moved,
  onChange,
}: {
  provider: AutoModProviderSettings;
  known: Shown;
  moved: boolean;
  onChange: (fn: (p: AutoModProviderSettings) => void) => void;
}) {
  const { t } = useI18n();
  const placeholder =
    moved && provider.apiKeySet
      ? t("instancesettings.moderation.retypeKey")
      : provider.apiKeySet
        ? t("instancesettings.moderation.savedEnds", { hint: provider.apiKeyHint || "••••" })
        : isCustom(provider)
          ? t("instancesettings.moderation.noneOrPaste")
          : t("instancesettings.moderation.paste");
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-sm font-extrabold">{t(provider.id === "cloudflare-clef" ? "instancesettings.moderation.apiToken" : "instancesettings.moderation.apiKey")}</span>
      <span className="text-xs text-muted-foreground">{known.keyHelp}</span>
      <span className="relative">
        <KeyRoundIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={provider.apiKey}
          onChange={(e) => onChange((p) => (p.apiKey = e.target.value))}
          placeholder={placeholder}
          className="h-10 rounded-xl pl-9"
        />
      </span>
      <span className="text-[0.7rem] text-muted-foreground">{t("instancesettings.moderation.kept")}</span>
    </label>
  );
}

/** Cloudflare's account id, which its address needs. */
function AccountField({ provider, onChange }: { provider: AutoModProviderSettings; onChange: (fn: (p: AutoModProviderSettings) => void) => void }) {
  const { t } = useI18n();
  const privateField = usePrivateField();
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-sm font-extrabold">{t("instancesettings.moderation.accountId")}</span>
      <Input
        value={provider.accountId}
        spellCheck={false}
        maxLength={32}
        onChange={(e) => onChange((p) => (p.accountId = e.target.value.trim()))}
        placeholder={t("instancesettings.moderation.accountIdPlaceholder")}
        className={cn("h-10 rounded-xl font-mono text-sm", privateField)}
      />
    </label>
  );
}

/** Which of a known provider's models to ask; the first is the default. */
function ModelChoice({ provider, known, onChange }: { provider: AutoModProviderSettings; known: Shown; onChange: (fn: (p: AutoModProviderSettings) => void) => void }) {
  const { t } = useI18n();
  const model = provider.model || known.models[0]?.id;
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-sm font-extrabold">{t("instancesettings.moderation.model")}</span>
      <div className="flex gap-1 rounded-2xl bg-muted/60 p-1" role="radiogroup" aria-label={t("instancesettings.moderation.modelOf", { name: known.name })}>
        {known.models.map((m) => {
          const on = m.id === model;
          return (
            <button
              key={m.id}
              type="button"
              role="radio"
              aria-checked={on}
              onClick={() => onChange((p) => (p.model = m.id === known.models[0].id ? "" : m.id))}
              className={cn("relative flex-1 rounded-xl px-3 py-1.5 text-left transition-colors", on ? "text-foreground" : "text-muted-foreground hover:text-foreground")}
            >
              {on && <motion.span layoutId={`model-${provider.id}`} transition={SPRING} className="absolute inset-0 rounded-xl bg-background shadow-sm" />}
              <span className="relative block text-sm font-bold">{m.label}</span>
              <span className="relative block text-[0.7rem]">{m.hint}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

/** Trying the provider on a sample, and what it answered; or what it still needs first. */
function TestBox({ instanceKey, provider, ready, missing }: { instanceKey: string; provider: AutoModProviderSettings; ready: boolean; missing: Key }) {
  const { t } = useI18n();
  const { testing, result, test } = useProviderTest(instanceKey, provider);
  return (
    <div className="flex flex-col gap-2 rounded-2xl bg-muted/50 p-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="flex flex-1 items-center gap-1.5 text-sm font-extrabold whitespace-nowrap">
          <FlaskConicalIcon className="size-4 text-primary" /> {t("instancesettings.moderation.test")}
        </span>
        <Button type="button" size="sm" variant="outline" className="rounded-xl font-bold" disabled={!ready || testing} onClick={() => void test()}>
          {testing ? <LoaderCircleIcon className="animate-spin" /> : <SparklesIcon />}
          {t(testing ? "instancesettings.moderation.asking" : "instancesettings.moderation.trySample")}
        </Button>
      </div>
      <AnimatePresence mode="popLayout" initial={false}>
        {result && (
          <motion.div
            key={result.ok ? "ok" : "failed"}
            initial={{ opacity: 0, y: 6, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -6 }}
            transition={SPRING}
            className="flex flex-col gap-2"
          >
            <TestAnswer result={result} />
          </motion.div>
        )}
      </AnimatePresence>
      {!ready && <span className="text-xs text-muted-foreground">{t(missing)}</span>}
    </div>
  );
}

/** How long the provider took and its top scores, or why it failed. */
function TestAnswer({ result }: { result: TestResult }) {
  const { t } = useI18n();
  if (!result.ok) {
    return (
      <span className="flex items-start gap-1.5 text-xs text-amber-700 dark:text-amber-400">
        <TriangleAlertIcon className="mt-0.5 size-3.5 shrink-0" />
        <span className="first-letter:uppercase">{result.error}</span>
      </span>
    );
  }
  return (
    <>
      <span className="flex items-center gap-1.5 text-xs font-bold text-emerald-600 dark:text-emerald-400">
        <CheckIcon className="size-3.5" strokeWidth={3} /> {t("instancesettings.moderation.answered", { ms: String(result.elapsedMs) })}
      </span>
      {result.scores.slice(0, 4).map((score, n) => (
        <div key={score.label} className="flex items-center gap-2 text-xs">
          <span className="w-24 shrink-0 truncate font-bold">{labelName(score.label)}</span>
          <span className="h-2 flex-1 overflow-hidden rounded-full bg-background">
            <motion.span
              initial={{ scaleX: 0 }}
              animate={{ scaleX: score.probability }}
              transition={{ ...SPRING, delay: 0.05 + n * 0.06 }}
              style={{ originX: 0 }}
              className={cn("block h-full w-full rounded-full", score.probability >= 0.8 ? "bg-destructive" : "bg-primary/60")}
            />
          </span>
          <span className="w-9 shrink-0 text-right tabular-nums">{Math.round(score.probability * 100)}%</span>
        </div>
      ))}
    </>
  );
}

/** A label's id as words: "self_harm" reads "Self harm". */
const labelName = (id: string) => (id.charAt(0).toUpperCase() + id.slice(1)).replaceAll("_", " ");
