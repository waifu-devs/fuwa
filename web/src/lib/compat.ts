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

/** What an app that needs updating tells people, or null when it's fine. */
export function updateLine(versions: VersionsInfo | undefined): string | null {
  const need = missing(versions);
  if (need.length) {
    const first = need[0]!.title;
    return need.length === 1 ? `Update fuwa to use ${first}` : `Update fuwa to use ${first} and ${need.length - 1} more`;
  }
  return tooOld(versions) ? "Update fuwa so everything works" : null;
}
