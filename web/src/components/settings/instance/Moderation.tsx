import { clone, create } from "@bufbuild/protobuf";
import { CheckIcon, FlaskConicalIcon, GlobeLockIcon, KeyRoundIcon, LoaderCircleIcon, PlusIcon, ShieldAlertIcon, SparklesIcon, Trash2Icon, TriangleAlertIcon, WebhookIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
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
import { cn } from "@/lib/utils";
import { Setting, SPRING } from "../controls";

type Reset = { changed: boolean; onReset: () => void; resetting: boolean };

/** What fuwa knows about each provider, beyond what the instance sends. */
const KNOWN: Record<string, { name: string; host: string; blurb: string; models: { id: string; label: string; hint: string }[]; keyHelp: string; tint: string }> = {
  "typesafe-jev": {
    name: "TypeSafe Jev",
    host: "api.typesafe.ai",
    blurb: "A decision model that answers yes-or-no questions about a text with a calibrated probability. Text only, run in the US.",
    models: [
      { id: "jev-latest", label: "Jev", hint: "The current release" },
      { id: "jev-preview", label: "Jev preview", hint: "The next release, early" },
    ],
    keyHelp: "An API key from your TypeSafe account. Billed per word read, by TypeSafe.",
    tint: "from-sky-500/25 to-indigo-500/10 text-sky-600 dark:text-sky-300",
  },
  "cloudflare-clef": {
    name: "Cloudflare Clef",
    host: "api.cloudflare.com",
    blurb: "Cloudflare's open decision models on Workers AI, answering the same questions as Jev. Runs on Cloudflare's network.",
    models: [
      { id: "@cf/cloudflare/clef", label: "Clef", hint: "Most accurate" },
      { id: "@cf/cloudflare/clef-flash", label: "Clef flash", hint: "Fastest, cheapest" },
    ],
    keyHelp: "An API token with Workers AI permission (Cloudflare dashboard, My Profile, API Tokens), and your account id from the dashboard's sidebar. Billed by Cloudflare.",
    tint: "from-orange-500/25 to-amber-500/10 text-orange-600 dark:text-orange-300",
  },
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

const fingerprint = (p: AutoModProviderSettings) => [
  p.id,
  p.enabled,
  p.apiKey.trim(),
  p.model.trim(),
  p.accountId.trim().toLowerCase(),
  p.name.trim(),
  p.url.trim(),
  p.header.trim().toLowerCase(),
];

/** The instance settings the Moderation page reads. */
export const MODERATION_FIELDS: { path: string; get: (s: InstanceSettings) => unknown; copy: (into: InstanceSettings, from: InstanceSettings) => void }[] = [
  {
    path: "automod_providers",
    get: (s) => JSON.stringify(s.automodProviders.map(fingerprint)),
    copy: (into, from) => (into.automodProviders = from.automodProviders.map((p) => clone(AutoModProviderSettingsSchema, p))),
  },
];

export const MODERATION_SECTION = {
  id: "moderation",
  label: "Moderation",
  icon: ShieldAlertIcon,
  description: "Services servers' AutoMod can ask about messages.",
  keywords: "automod ai jev typesafe cloudflare clef workers smart filter custom webhook own",
  settings: [
    { id: "automod-typesafe-jev", label: "TypeSafe Jev", keywords: "automod ai moderation key" },
    { id: "automod-cloudflare-clef", label: "Cloudflare Clef", keywords: "automod ai moderation workers token account" },
    { id: "automod-custom", label: "Your own provider", keywords: "automod custom webhook classifier own endpoint" },
  ],
};

/**
 * Moderation services, set up once for the whole instance: a key and a
 * switch each. Servers then pick one for their AutoMod's smart filter.
 */
export function ModerationSettings({
  instanceKey,
  draft,
  saved,
  patch,
  resetter,
}: {
  instanceKey: string;
  draft: InstanceSettings;
  saved: InstanceSettings;
  patch: (fn: (d: InstanceSettings) => void) => void;
  resetter: (...paths: string[]) => Reset;
}) {
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
        <p className="text-muted-foreground">
          Turn a service on here and every server on this instance can pick it for AutoMod's smart filter, with one switch. Only this instance talks to it, and only
          about messages in servers that turned the filter on: their text, never who wrote them, where, or the server's name. If it's slow or down, messages go
          through and the servers' own rules still apply.
        </p>
      </motion.div>
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
            <span className="block font-extrabold">Add your own</span>
            <span className="block text-xs text-muted-foreground">
              {customs >= MAX_CUSTOM
                ? `That's ${MAX_CUSTOM}, the most an instance keeps.`
                : "A classifier you run, or any https address that answers the same questions as Jev and Clef. What it gets and answers is in docs/automod.md."}
            </span>
          </span>
        </motion.button>
      </motion.div>
    </>
  );
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
  const privateField = usePrivateField();
  const custom = isCustom(provider);
  const host = custom ? hostOf(provider.url) : "";
  const known = custom
    ? {
        name: provider.name.trim() || "Your provider",
        host: host || "an address you pick",
        blurb: "Yours, at an https address you pick. It gets the same questions as Jev and Clef.",
        models: [],
        keyHelp: "If it needs one. It goes as Authorization: Bearer, or in the header you name.",
        tint: "from-violet-500/25 to-fuchsia-500/10 text-violet-600 dark:text-violet-300",
      }
    : (KNOWN[provider.id] ?? { name: provider.id, host: "", blurb: "", models: [], keyHelp: "", tint: "" });
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<TestAutoModProviderResponse | { ok: false; error: string; elapsedMs: number; scores: [] } | null>(null);
  // A saved key stays with its address: a new address needs it typed again.
  const moved = custom && !!saved && saved.url.trim() !== provider.url.trim();
  const hasKey = (provider.apiKeySet && !moved) || provider.apiKey.trim() !== "";
  const needsAccount = provider.id === "cloudflare-clef";
  const ready = custom
    ? provider.name.trim() !== "" && host !== "" && (provider.header.trim() === "" || hasKey)
    : hasKey && (!needsAccount || provider.accountId.trim().length === 32);
  const model = provider.model || known.models[0]?.id;
  const live = saved?.enabled ?? false;

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

  return (
    <Setting
      id={`automod-${provider.id}`}
      title={known.name}
      delay={delay}
      badge={!!reset}
      defaultLabel="off"
      changed={reset?.changed}
      onReset={reset?.onReset}
      resetting={reset?.resetting}
    >
      <div className={cn("overflow-hidden rounded-3xl border transition-colors", provider.enabled ? "border-primary/40 bg-primary/[0.03]" : "", custom && !provider.enabled && "border-dashed")}>
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
                    Servers can use it
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
            aria-label={provider.enabled ? `Turn ${known.name} off` : `Turn ${known.name} on`}
          />
        </label>
        <div className="flex flex-col gap-3 border-t p-4">
          <div className="flex items-start gap-2.5 rounded-2xl bg-muted/60 p-3 text-xs text-muted-foreground">
            <GlobeLockIcon className="mt-0.5 size-4 shrink-0 text-primary" />
            <span>
              Turned on, messages that servers choose to check go from this instance to <b className="break-all text-foreground">{known.host}</b>, which sees their
              text. Nothing else goes: no names, ids, servers or addresses.
            </span>
          </div>
          {custom && (
            <>
              <div className="grid gap-3 sm:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
                <label className="flex flex-col gap-1.5">
                  <span className="text-sm font-extrabold">Name</span>
                  <Input
                    value={provider.name}
                    maxLength={40}
                    onChange={(e) => onChange((p) => (p.name = e.target.value))}
                    placeholder="What servers see, like Our classifier"
                    className="h-10 rounded-xl"
                  />
                </label>
                <label className="flex flex-col gap-1.5">
                  <span className="text-sm font-extrabold">Address</span>
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
                    fuwa only talks to https addresses, like https://moderation.example.com/v1/check.
                  </motion.span>
                )}
              </AnimatePresence>
              <div className="grid gap-3 sm:grid-cols-2">
                <label className="flex flex-col gap-1.5">
                  <span className="text-sm font-extrabold">Key header</span>
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
                  <span className="text-sm font-extrabold">Model</span>
                  <Input
                    value={provider.model}
                    spellCheck={false}
                    maxLength={100}
                    onChange={(e) => onChange((p) => (p.model = e.target.value))}
                    placeholder="Optional, sent as model"
                    className="h-10 rounded-xl font-mono text-sm"
                  />
                </label>
              </div>
            </>
          )}
          <label className="flex flex-col gap-1.5">
            <span className="text-sm font-extrabold">{needsAccount ? "API token" : "API key"}</span>
            <span className="text-xs text-muted-foreground">{known.keyHelp}</span>
            <span className="relative">
              <KeyRoundIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={provider.apiKey}
                onChange={(e) => onChange((p) => (p.apiKey = e.target.value))}
                placeholder={
                  moved && provider.apiKeySet
                    ? "New address: type the key again"
                    : provider.apiKeySet
                      ? `Saved, ends in ${provider.apiKeyHint || "••••"}. Type to replace it.`
                      : custom
                        ? "None, or paste it here"
                        : "Paste it here"
                }
                className="h-10 rounded-xl pl-9"
              />
            </span>
            <span className="text-[0.7rem] text-muted-foreground">Kept on the instance, sealed with its files when it encrypts them, and never shown again.</span>
          </label>
          {needsAccount && (
            <label className="flex flex-col gap-1.5">
              <span className="text-sm font-extrabold">Account id</span>
              <Input
                value={provider.accountId}
                spellCheck={false}
                maxLength={32}
                onChange={(e) => onChange((p) => (p.accountId = e.target.value.trim()))}
                placeholder="32 letters and digits"
                className={cn("h-10 rounded-xl font-mono text-sm", privateField)}
              />
            </label>
          )}
          {known.models.length > 1 && (
            <div className="flex flex-col gap-1.5">
              <span className="text-sm font-extrabold">Model</span>
              <div className="flex gap-1 rounded-2xl bg-muted/60 p-1" role="radiogroup" aria-label={`${known.name} model`}>
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
          )}
          <div className="flex flex-col gap-2 rounded-2xl bg-muted/50 p-3">
            <div className="flex flex-wrap items-center gap-2">
              <span className="flex flex-1 items-center gap-1.5 text-sm font-extrabold whitespace-nowrap">
                <FlaskConicalIcon className="size-4 text-primary" /> Test connection
              </span>
              <Button type="button" size="sm" variant="outline" className="rounded-xl font-bold" disabled={!ready || testing} onClick={() => void test()}>
                {testing ? <LoaderCircleIcon className="animate-spin" /> : <SparklesIcon />}
                {testing ? "Asking…" : "Try a sample scam"}
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
                  {result.ok ? (
                    <>
                      <span className="flex items-center gap-1.5 text-xs font-bold text-emerald-600 dark:text-emerald-400">
                        <CheckIcon className="size-3.5" strokeWidth={3} /> It answered in {result.elapsedMs} ms
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
                  ) : (
                    <span className="flex items-start gap-1.5 text-xs text-amber-700 dark:text-amber-400">
                      <TriangleAlertIcon className="mt-0.5 size-3.5 shrink-0" />
                      <span className="first-letter:uppercase">{result.error}</span>
                    </span>
                  )}
                </motion.div>
              )}
            </AnimatePresence>
            {!ready && (
              <span className="text-xs text-muted-foreground">
                {custom
                  ? `Add ${[provider.name.trim() ? "" : "a name", host ? "" : "an https address", provider.header.trim() && !hasKey ? "the key" : ""].filter(Boolean).join(" and ")} to test it and turn it on.`
                  : `Add the ${needsAccount ? "token and account id" : "key"} to test it and turn it on.`}
              </span>
            )}
          </div>
          {onRemove && (
            <Button type="button" variant="ghost" size="sm" className="self-start rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive" onClick={onRemove}>
              <Trash2Icon /> Remove {provider.name.trim() || "this provider"}
            </Button>
          )}
        </div>
      </div>
    </Setting>
  );
}

/** A label's id as words: "self_harm" reads "Self harm". */
const labelName = (id: string) => (id.charAt(0).toUpperCase() + id.slice(1)).replaceAll("_", " ");
