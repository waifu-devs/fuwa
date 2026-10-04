/*
 * Files attached to messages: what each one is shown as, and the small
 * decisions the composer and the message list share. Only plain functions,
 * so node's test runner reads this file as is.
 */

/** At most this many files go with one message, as on the server. */
export const MAX_FILES = 10;

/** How a file is shown in a message. */
export type FileLook = "picture" | "video" | "audio" | "file";

/** What the server serves as pictures, and what browsers play. */
const PICTURES = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/avif"];

/** How a sent file is shown, by the kind the server found in its bytes. */
export function lookOf(contentType: string): FileLook {
  if (PICTURES.includes(contentType)) return "picture";
  if (contentType.startsWith("video/")) return "video";
  if (contentType.startsWith("audio/")) return "audio";
  return "file";
}

/** How a file about to be sent is shown, by what the browser says it is. */
export function localLook(file: { type: string; name: string }): FileLook {
  if (PICTURES.includes(file.type)) return "picture";
  if (/^video\/(mp4|webm|quicktime)$/.test(file.type)) return "video";
  if (/^audio\//.test(file.type)) return "audio";
  return "file";
}

/** What kind of file a name says it is, for its icon. */
export type FileFamily = "archive" | "code" | "document" | "sheet" | "slides" | "pdf" | "text" | "audio" | "video" | "picture" | "other";

const FAMILIES: [FileFamily, string[]][] = [
  ["archive", ["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst"]],
  ["code", ["js", "ts", "tsx", "jsx", "rs", "go", "py", "rb", "java", "kt", "c", "h", "cpp", "hpp", "cs", "swift", "php", "sh", "lua", "json", "toml", "yaml", "yml", "html", "css", "sql", "proto"]],
  ["pdf", ["pdf"]],
  ["document", ["doc", "docx", "odt", "rtf", "pages"]],
  ["sheet", ["xls", "xlsx", "ods", "csv", "tsv", "numbers"]],
  ["slides", ["ppt", "pptx", "odp", "key"]],
  ["text", ["txt", "md", "log", "ini", "cfg"]],
  ["audio", ["mp3", "wav", "flac", "ogg", "opus", "m4a", "aac"]],
  ["video", ["mp4", "webm", "mov", "mkv", "avi"]],
  ["picture", ["png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "heic", "bmp", "tif", "tiff"]],
];

/** The extension of a file's name, lowercased, without the dot; "" when it has none. */
export function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1).toLowerCase() : "";
}

export function familyOf(name: string): FileFamily {
  const ext = extensionOf(name);
  return FAMILIES.find(([, exts]) => exts.includes(ext))?.[0] ?? "other";
}

/**
 * A long name shortened in the middle so its end (the extension, often the
 * part that matters) still shows: "a-very-long-repo…-v2.zip".
 */
export function shortName(name: string, max = 40): string {
  const chars = Array.from(name);
  if (chars.length <= max) return name;
  const ext = extensionOf(name);
  const tail = Math.min(ext.length + 6, Math.floor(max / 2));
  return `${chars.slice(0, max - tail - 1).join("")}…${chars.slice(-tail).join("")}`;
}

/** The box a picture or video is shown in, at most `max` wide and tall, keeping its shape. */
export function fitBox(width: number, height: number, max: { width: number; height: number }) {
  if (!(width > 0 && height > 0)) return { width: max.width, height: Math.round(max.width * 0.5625) };
  const scale = Math.min(1, max.width / width, max.height / height);
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

/** Why the files can't be added, or "" when they can. */
export function cantAdd(already: number, adding: number): string {
  if (already + adding <= MAX_FILES) return "";
  return `You can send up to ${MAX_FILES} files at once`;
}
