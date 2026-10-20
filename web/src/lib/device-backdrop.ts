/**
 * A video or picture from this device behind the app (`Prefs.backdropFile`),
 * standing in for the backdrop's picture: the pure part, so it can be tested.
 * The file itself is kept in this browser (`backdrop-files.ts`) and never
 * leaves the device; the setting naming it doesn't travel either.
 */

/** A video or picture kept on this device, by its id there. */
export type BackdropFile = { id: string; kind: "video" | "image"; name: string };

/** The biggest file kept from this device for the backdrop. */
export const MOST_BACKDROP_FILE_BYTES = 100 * 1024 * 1024;

/** What the file picker offers. */
export const BACKDROP_FILE_ACCEPT = "video/mp4,video/webm,video/quicktime,image/*,.mp4,.m4v,.webm,.mov,.mkv";

/** A stored device file, or null when it isn't one. */
export function sanitizeBackdropFile(value: unknown): BackdropFile | null {
  const f = (value && typeof value === "object" ? value : {}) as Partial<Record<keyof BackdropFile, unknown>>;
  if (typeof f.id !== "string" || !/^[0-9a-f]{16,64}$/.test(f.id) || (f.kind !== "video" && f.kind !== "image")) return null;
  return { id: f.id, kind: f.kind, name: typeof f.name === "string" ? f.name.slice(0, 80) : "" };
}

/** What a file is, from its type or else its name; null for anything that isn't a video or picture. */
export function backdropFileKind(file: { type: string; name: string }): BackdropFile["kind"] | null {
  if (file.type.startsWith("video/")) return "video";
  if (file.type.startsWith("image/")) return file.type === "image/svg+xml" ? null : "image";
  if (file.type) return null;
  const ext = /\.([0-9a-z]+)$/.exec(file.name.toLowerCase())?.[1] ?? "";
  if (["mp4", "m4v", "webm", "mov", "mkv"].includes(ext)) return "video";
  if (["gif", "png", "jpg", "jpeg", "webp", "avif", "apng"].includes(ext)) return "image";
  return null;
}
