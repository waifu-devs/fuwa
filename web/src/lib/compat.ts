/**
 * Compatibility dates (docs/compatibility.md). `proto/fuwa/v1/features.json`
 * lists every feature an app has to know to use, each with the date it
 * arrived; `pnpm generate` copies it to `src/gen/features.json`, so this
 * build knows exactly the features it was made with. An instance says which
 * it has in `Node.versions`. Where the instance has a feature this app
 * doesn't know, the app says "update to use this" instead of showing
 * something broken, and everything else keeps working. Nothing is forced.
 */
import list from "../gen/features.json" with { type: "json" };

export type FeatureInfo = { id: string; date: string; title: string };
/** Just what's read of `Node.versions`, so plain objects work in tests. */
export type VersionsInfo = { compatibilityDate: string; minClientDate: string; features: FeatureInfo[] };

/** The features this build knows. */
export const FEATURES: FeatureInfo[] = list.features;
/** This build's compatibility date: its newest feature's. */
export const CLIENT_DATE = FEATURES.reduce((max, f) => (f.date > max ? f.date : max), "");
/** Instances from before compatibility dates had every feature dated this day or earlier. */
const BASELINE = "2026-10-04";

/** Features the instance has that this app doesn't know: they need a newer app. */
export function missing(versions: VersionsInfo | undefined, ours: FeatureInfo[] = FEATURES): FeatureInfo[] {
  if (!versions) return [];
  const known = new Set(ours.map((f) => f.id));
  return versions.features.filter((f) => !known.has(f.id));
}

/** Whether this app is older than the instance serves fully. */
export function tooOld(versions: VersionsInfo | undefined, clientDate: string = CLIENT_DATE): boolean {
  return !!versions?.minClientDate && clientDate < versions.minClientDate;
}

/**
 * Whether the instance has a feature this app knows, so its screens may
 * show. An instance from before compatibility dates has the baseline ones.
 */
export function instanceHas(versions: VersionsInfo | undefined, id: string, ours: FeatureInfo[] = FEATURES): boolean {
  if (versions) return versions.features.some((f) => f.id === id);
  const feature = ours.find((f) => f.id === id);
  return !!feature && feature.date <= BASELINE;
}

/** The most of a feature title or instance name shown. */
const MAX_SHOWN = 40;

/**
 * A title or name from an instance, made safe to show in an update notice:
 * letters, digits, spaces and a little punctuation, never anything that reads
 * as a link or an address, and short.
 */
export function shown(text: string, fallback: string): string {
  const plain = text.replace(/[^\p{L}\p{N} '&(),-]/gu, " ").replace(/\s+/g, " ").trim();
  if (!plain) return fallback;
  return plain.length > MAX_SHOWN ? `${plain.slice(0, MAX_SHOWN - 1).trimEnd()}…` : plain;
}

/**
 * What an app that needs updating tells people, naming the instance it's
 * about, or null when it's fine.
 */
export function updateLine(versions: VersionsInfo | undefined, instance: string): string | null {
  const name = shown(instance, "An instance");
  const need = missing(versions);
  if (need.length) {
    const first = shown(need[0]!.title, "something new");
    return need.length === 1
      ? `${name} has ${first}. Update fuwa to use it`
      : `${name} has ${first} and ${need.length - 1} more. Update fuwa to use them`;
  }
  return tooOld(versions) ? `${name} needs a newer fuwa for everything to work` : null;
}
