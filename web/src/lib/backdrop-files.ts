import { backdropFileKind, MOST_BACKDROP_FILE_BYTES, type BackdropFile } from "./device-backdrop";

/**
 * The backdrop's video or picture from this device (`Prefs.backdropFile`),
 * kept in this browser's IndexedDB and played from there, so nothing is
 * fetched to draw it. It never leaves the device: nothing here talks to an
 * instance, and the setting that names it doesn't travel.
 */

const DB = "fuwa-backdrops";
const STORE = "files";

type Kept = { id: string; blob: Blob };

let opening: Promise<IDBDatabase> | undefined;

function db(): Promise<IDBDatabase> {
  opening ??= new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, 1);
    req.onupgradeneeded = () => req.result.createObjectStore(STORE, { keyPath: "id" });
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => {
      opening = undefined;
      reject(req.error);
    };
  });
  return opening;
}

async function run<T>(mode: IDBTransactionMode, work: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const tx = (await db()).transaction(STORE, mode);
  const req = work(tx.objectStore(STORE));
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve(req.result);
    tx.onerror = tx.onabort = () => reject(tx.error ?? req.error);
  });
}

export type BackdropFileError = { error: "notMedia" | "tooBig" | "cantSave" };

/** Whether this browser draws it: a video's first frame, a picture decoded. */
function plays(url: string, kind: BackdropFile["kind"]): Promise<boolean> {
  return new Promise((resolve) => {
    const done = (ok: boolean) => {
      clearTimeout(timer);
      resolve(ok);
    };
    const timer = setTimeout(() => done(false), 15_000);
    if (kind === "image") {
      const image = new Image();
      image.onload = () => done(image.naturalWidth > 0);
      image.onerror = () => done(false);
      image.src = url;
      return;
    }
    const video = document.createElement("video");
    video.muted = true;
    video.preload = "auto";
    video.onloadeddata = () => done(video.videoWidth > 0);
    video.onerror = () => done(false);
    video.src = url;
  });
}

/** Keeps a video or picture on this device once the browser can draw it: the setting to save, or what went wrong. */
export async function importBackdropFile(file: File): Promise<BackdropFile | BackdropFileError> {
  const kind = backdropFileKind(file);
  if (!kind) return { error: "notMedia" };
  if (file.size > MOST_BACKDROP_FILE_BYTES) return { error: "tooBig" };
  const probe = URL.createObjectURL(file);
  try {
    if (!(await plays(probe, kind))) return { error: "notMedia" };
  } finally {
    URL.revokeObjectURL(probe);
  }
  const id = [...crypto.getRandomValues(new Uint8Array(12))].map((b) => b.toString(16).padStart(2, "0")).join("");
  try {
    await run("readwrite", (s) => s.put({ id, blob: file.slice(0, file.size, file.type) } satisfies Kept));
  } catch {
    return { error: "cantSave" };
  }
  return { id, kind, name: file.name.slice(0, 80) };
}

/** The one link made for a kept file at a time: the backdrop and its previews share it. */
let shown: { id: string; url: Promise<string | null> } | null = null;

/** A link to a kept file to draw it from; null when it's gone from this browser. */
export function backdropFileUrl(id: string): Promise<string | null> {
  if (shown?.id === id) return shown.url;
  const before = shown;
  const url = (async () => {
    try {
      const kept = await run<Kept | undefined>("readonly", (s) => s.get(id));
      return kept ? URL.createObjectURL(kept.blob) : null;
    } catch {
      return null;
    }
  })();
  shown = { id, url };
  // The last one's link is let go a moment later, once nothing still draws it.
  if (before) void before.url.then((old) => old && setTimeout(() => URL.revokeObjectURL(old), 5_000));
  return url;
}

/** Deletes kept files no setting uses any more. */
export async function sweepBackdropFiles(keep: string[]) {
  try {
    const ids = await run<IDBValidKey[]>("readonly", (s) => s.getAllKeys());
    const gone = ids.filter((id) => typeof id === "string" && !keep.includes(id));
    if (gone.length) await run("readwrite", (s) => (gone.forEach((id) => s.delete(id)), s.count()));
  } catch {
    // Kept for now; swept next time.
  }
}
