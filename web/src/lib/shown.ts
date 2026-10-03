import { store } from "@/fuwa/store";
import { loadSaved, normalizeUrl } from "@/fuwa/saved";

/*
 * Pictures are only ever loaded from fuwa instances: this one, the ones you
 * added, and any you chose to open an invite on. A picture anywhere else
 * would tell that site your IP address and when you looked, so it isn't
 * loaded at all. Instances fetch pictures from other sites for you (their
 * links point at /media/outside/ on the instance), so this only hides links
 * stored before they did, or sent by an instance that doesn't.
 */

const chosen = new Set<string>();

const originOf = (url: string) => {
  try {
    return new URL(url).origin;
  } catch {
    return "";
  }
};

/** Lets pictures load from an instance someone chose to open, like one an invite is on. */
export function allowPicturesFrom(address: string) {
  try {
    chosen.add(new URL(normalizeUrl(address)).origin);
  } catch {
    // Not an address; nothing to allow.
  }
}

/** Every origin pictures may load from right now. */
function trusted(): Set<string> {
  const origins = new Set(chosen);
  if (typeof location !== "undefined") origins.add(location.origin);
  for (const saved of loadSaved()) origins.add(originOf(saved.url));
  for (const inst of Object.values(store.get().instances)) {
    origins.add(originOf(inst.url));
    if (inst.node?.publicUrl) origins.add(originOf(inst.node.publicUrl));
  }
  origins.delete("");
  return origins;
}

/** `src` if it's a picture on a fuwa instance, else "" (so the fallback shows). */
export function shownPicture(src: string | undefined): string {
  if (!src) return "";
  const origin = originOf(src);
  return origin && trusted().has(origin) ? src : "";
}
