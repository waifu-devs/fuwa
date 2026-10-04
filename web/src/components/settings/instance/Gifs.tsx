import { clone, create } from "@bufbuild/protobuf";
import { CircleCheckIcon, CircleXIcon, FilmIcon, KeyRoundIcon, LoaderCircleIcon, PowerOffIcon, ShieldIcon, SparklesIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
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
import { cn } from "@/lib/utils";
import { Cap, Choice, Setting, SPRING } from "../controls";

type Reset = { changed: boolean; onReset: () => void; resetting: boolean };

const gifsOf = (s: InstanceSettings) => s.gifs ?? create(GifSettingsSchema);

/** The instance settings this page reads: all of GIFs is one setting. */
export const GIF_FIELDS: { path: string; get: (s: InstanceSettings) => unknown; copy: (into: InstanceSettings, from: InstanceSettings) => void }[] = [
  {
    path: "gifs",
    get: (s) => {
      const g = gifsOf(s);
      // The key is never sent back: an empty field keeps the saved one.
      return [g.provider, g.apiKey.trim(), g.rating || "pg-13", g.gifBytes, g.searchesPerMinute, g.providerCallsPerDay].join("|");
    },
    copy: (into, from) => (into.gifs = clone(GifSettingsSchema, gifsOf(from))),
  },
];

export const GIF_SECTION = {
  id: "gifs",
  label: "GIFs",
  icon: FilmIcon,
  description: "GIF search in the composer, through this instance.",
  keywords: "gif giphy klipy tenor search animated",
  settings: [
    { id: "gif-provider", label: "GIF search provider", keywords: "giphy klipy" },
    { id: "gif-key", label: "Provider key", keywords: "api key secret" },
    { id: "gif-rating", label: "Rating", keywords: "nsfw safe content filter" },
    { id: "gif-caps", label: "GIF caps", keywords: "size limit rate searches per day" },
  ],
};

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
  const g = gifsOf(draft);
  const was = gifsOf(saved);
  const def = gifsOf(defaults);
  const edit = (fn: (g: GifSetup) => void) =>
    patch((d) => {
      d.gifs = clone(GifSettingsSchema, gifsOf(d));
      fn(d.gifs);
    });
  const reset = resetter("gifs");
  const privateField = usePrivateField();
  const name = g.provider === GifProvider.GIPHY ? "GIPHY" : g.provider === GifProvider.KLIPY ? "Klipy" : "";
  const keyMissing = g.provider !== GifProvider.UNSPECIFIED && !g.apiKey.trim() && !(was.apiKeySet && was.provider === g.provider);

  return (
    <>
      <Setting
        id="gif-provider"
        title="GIF search provider"
        hint="Where the GIF button's search, trending and moods come from. This instance asks the provider for everyone and hands back every picture itself, so the provider never sees anyone's address, account or what they send, and apps load nothing from it. A GIF someone sends is stored here, without its metadata, so it stays after the provider drops it."
        defaultLabel={providerLabel(def.provider)}
        {...reset}
      >
        <Choice
          value={g.provider}
          onChange={(provider) => edit((x) => (x.provider = provider))}
          options={[
            { value: GifProvider.UNSPECIFIED, label: "Off", hint: "No GIF button. GIFs already sent still show.", icon: <PowerOffIcon className="size-4" /> },
            { value: GifProvider.GIPHY, label: "GIPHY", hint: "Recommended: the biggest library. Free key at developers.giphy.com.", icon: <SparklesIcon className="size-4" /> },
            { value: GifProvider.KLIPY, label: "Klipy", hint: "A Tenor-style library. Free key at partner.klipy.com.", icon: <FilmIcon className="size-4" /> },
          ]}
        />
      </Setting>
      <AnimatePresence initial={false}>
        {g.provider !== GifProvider.UNSPECIFIED && (
          <motion.div key="on" initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 10 }} transition={SPRING}>
            <Setting
              id="gif-key"
              title={`${name} key`}
              hint={`Kept on this instance (encrypted with its files when it encrypts them) and never sent back to any app, this page included. ${name}'s terms ask for "Powered by ${name}" by results; the picker shows it.`}
              delay={0.02}
              defaultLabel={def.apiKeySet ? "set" : "none"}
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
                    was.apiKeySet && was.provider === g.provider
                      ? was.apiKeyHint
                        ? `Saved, ending in ${was.apiKeyHint}. Type to replace it`
                        : "Saved. Type to replace it"
                      : `Paste your ${name} key`
                  }
                  className={cn("h-10 rounded-xl pl-9 font-mono text-sm", privateField, keyMissing && "ring-2 ring-amber-500/50")}
                />
              </div>
              {keyMissing && <p className="text-xs text-amber-600 dark:text-amber-400">GIFs stay off until a key is saved.</p>}
              <TryIt instanceKey={instanceKey} settings={g} />
            </Setting>
            <Setting
              id="gif-rating"
              title="Rating"
              hint="The most a result may be rated. The provider decides ratings; AutoMod's picture checks still look at what's sent."
              delay={0.04}
              defaultLabel={RATING_LABEL[def.rating || "pg-13"]}
              {...reset}
            >
              <Choice
                value={g.rating || "pg-13"}
                onChange={(rating) => edit((x) => (x.rating = rating))}
                options={[
                  { value: "g", label: "G", hint: "For everyone.", icon: <ShieldIcon className="size-4" /> },
                  { value: "pg", label: "PG", hint: "Mild.", icon: <ShieldIcon className="size-4" /> },
                  { value: "pg-13", label: "PG-13", hint: "The usual.", icon: <ShieldIcon className="size-4" /> },
                  { value: "r", label: "R", hint: "Anything but adult.", icon: <ShieldIcon className="size-4" /> },
                ]}
              />
            </Setting>
            <Setting
              id="gif-caps"
              title="Caps"
              hint="Off means no cap. Searches kept from the last ten minutes don't count toward the daily calls."
              delay={0.06}
              defaultLabel="no caps"
              {...reset}
            >
              <div className="grid gap-3 sm:grid-cols-3">
                <Cap label="Largest GIF stored" bytes value={g.gifBytes} onChange={(v) => edit((x) => (x.gifBytes = v))} />
                <Cap label="Searches a minute, each" value={g.searchesPerMinute} onChange={(v) => edit((x) => (x.searchesPerMinute = v))} />
                <Cap label="Calls to the provider a day" value={g.providerCallsPerDay} onChange={(v) => edit((x) => (x.providerCallsPerDay = v))} />
              </div>
              {g.gifBytes !== undefined && (
                <p className="text-xs text-muted-foreground">
                  Bigger GIFs are stored at the provider's smaller size, or refused when even that is over {formatBytes(Number(g.gifBytes))}.
                </p>
              )}
            </Setting>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  );
}

function providerLabel(p: GifProvider) {
  return p === GifProvider.GIPHY ? "GIPHY" : p === GifProvider.KLIPY ? "Klipy" : "off";
}

/** Asks the provider for a few trending GIFs with what's typed, before it's saved. */
function TryIt({ instanceKey, settings }: { instanceKey: string; settings: GifSetup }) {
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
          Try it
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
              {result.ok ? `Works · ${result.elapsedMs} ms` : result.error}
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
