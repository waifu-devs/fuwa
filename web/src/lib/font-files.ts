import { cleanFamily, familiesOf, fileFamily, fontFileName, MOST_FONT_BYTES, quoteFamily, type FontPick } from "./fonts";

/**
 * Font files imported on this device, kept in this browser's IndexedDB by
 * what they hold and put on the page as FontFaces. They never leave the
 * device: nothing here talks to an instance.
 */

const DB = "fuwa-fonts";
const STORE = "files";

type Kept = { id: string; bytes: ArrayBuffer };

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

export type FontError = { error: "notFont" | "tooBig" | "cantSave" };

const hex = (bytes: ArrayBuffer) => [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");

/** Keeps a font file on this device once the browser can draw with it: the pick to save, or what went wrong. */
export async function importFont(file: File): Promise<FontPick | FontError> {
  if (file.size > MOST_FONT_BYTES) return { error: "tooBig" };
  if (!fontFileName(file.name)) return { error: "notFont" };
  const bytes = await file.arrayBuffer();
  const id = hex(await crypto.subtle.digest("SHA-256", bytes)).slice(0, 24);
  try {
    // Only what the browser takes as a font gets kept.
    await new FontFace(fileFamily(id), bytes.slice(0)).load();
  } catch {
    return { error: "notFont" };
  }
  try {
    await run("readwrite", (s) => s.put({ id, bytes } satisfies Kept));
  } catch {
    return { error: "cantSave" };
  }
  return { kind: "file", id, name: file.name.slice(0, 80) };
}

const loaded = new Map<string, Promise<boolean>>();

/** Puts a kept file's font on the page (once); false when it's gone from this browser. */
export function loadFontFile(id: string): Promise<boolean> {
  let done = loaded.get(id);
  if (!done) {
    done = (async () => {
      try {
        const kept = await run<Kept | undefined>("readonly", (s) => s.get(id));
        if (!kept) return false;
        const face = new FontFace(fileFamily(id), kept.bytes);
        document.fonts.add(await face.load());
        return true;
      } catch {
        return false;
      }
    })();
    loaded.set(id, done);
    // A failure can be tried again, after the file's imported again.
    void done.then((ok) => ok || loaded.delete(id));
  }
  return done;
}

/** Deletes kept files no setting uses any more. */
export async function sweepFontFiles(keep: string[]) {
  try {
    const ids = await run<IDBValidKey[]>("readonly", (s) => s.getAllKeys());
    const gone = ids.filter((id) => typeof id === "string" && !keep.includes(id));
    if (gone.length) await run("readwrite", (s) => (gone.forEach((id) => s.delete(id)), s.count()));
  } catch {
    // Kept for now; swept next time.
  }
}

type LocalFonts = () => Promise<{ family: string }[]>;

/** Whether this browser can list the fonts on this computer (Chromium's Local Font Access). */
export const canListFonts = () => typeof window !== "undefined" && "queryLocalFonts" in window;

/** The fonts on this computer, after the browser asks; empty if it can't or was told no. */
export async function listFonts(): Promise<string[]> {
  try {
    const query = (window as unknown as { queryLocalFonts?: LocalFonts }).queryLocalFonts;
    return query ? familiesOf(await query()) : [];
  } catch {
    return [];
  }
}

let canvas: CanvasRenderingContext2D | null | undefined;

/**
 * Whether a font by this name is on this device: drawn with it, a line comes
 * out another width than with both generic families it would fall back to.
 */
export function hasFont(family: string): boolean {
  const name = cleanFamily(family);
  if (!name || typeof document === "undefined") return true;
  canvas ??= document.createElement("canvas").getContext("2d");
  if (!canvas) return true;
  const sample = "mmmmmmmmmmlli1WQ@ふわ";
  const quoted = quoteFamily(name);
  return ["monospace", "serif"].some((generic) => {
    canvas!.font = `32px ${generic}`;
    const plain = canvas!.measureText(sample).width;
    canvas!.font = `32px ${quoted}, ${generic}`;
    return canvas!.measureText(sample).width !== plain;
  });
}
