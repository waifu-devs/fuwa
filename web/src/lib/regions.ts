// Where an instance keeps community servers (docs/regions.md). A server's
// `region` is empty for the home region, and so is a region's `id` on an
// instance that never named its home.

export type RegionLike = { id: string; name: string; home: boolean };

/** Whether there's a region to pick: more than one. */
export const hasRegions = (regions: readonly RegionLike[] | undefined) => (regions?.length ?? 0) > 1;

/** The region a server is in, from its `region` label. */
export function regionOf(regions: readonly RegionLike[] | undefined, region: string): RegionLike | undefined {
  const list = regions ?? [];
  return list.find((r) => (region ? r.id === region : r.home)) ?? (region ? undefined : list[0]);
}

/** A region's name for people: its own, or the label when the instance doesn't list it. */
export function regionName(regions: readonly RegionLike[] | undefined, region: string): string {
  return regionOf(regions, region)?.name || region || "Home";
}

/** Whether `region` names the same place as the server's `current` one. */
export const sameRegion = (regions: readonly RegionLike[] | undefined, region: string, current: string) =>
  regionOf(regions, region)?.id === regionOf(regions, current)?.id && (regionOf(regions, region) !== undefined || region === current);

/** A small flag-free mark for a region: its first letters, as in "EU" or "US". */
export function regionMark(name: string): string {
  const words = name.split(/\s+/).filter(Boolean);
  if (/^[A-Z]{2,3}$/.test(words[0] ?? "")) return words[0]!;
  if (words.length > 1) return words.map((w) => w[0]).join("").slice(0, 2).toUpperCase();
  return (words[0] ?? "?").slice(0, 2).toUpperCase();
}
