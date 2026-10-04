/**
 * Emoji: a server's own are written `<:name:id>`, or `<a:name:id>` when they
 * move. What the pickers offer is in lib/emoji-catalog.ts.
 */

import type { Emoji } from "@/gen/fuwa/v1/types_pb";

/** A server emoji as it's written in a message. */
export const EMOJI_TOKEN = /<(a?):([A-Za-z0-9_]{2,32}):([0-9A-Za-z]{10,32})>/g;

/** How a server emoji is written in a message. */
export const emojiToken = (emoji: Pick<Emoji, "animated" | "name" | "id">) => `<${emoji.animated ? "a" : ""}:${emoji.name}:${emoji.id}>`;

/** The name rule the server keeps: 2 to 32 letters, digits and underscores. */
export const EMOJI_NAME = /^[A-Za-z0-9_]{2,32}$/;

/** A file name made into an emoji name: `Party Cat.png` → `party_cat`. */
export function nameFromFile(file: string) {
  const base = file
    .replace(/\.[^.]+$/, "")
    .toLowerCase()
    .replace(/[^a-z0-9_]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 32);
  return base.length >= 2 ? base : `emoji_${base || Math.floor(Math.random() * 1000)}`;
}

const PICTOGRAPH = /\p{Extended_Pictographic}/u;

/**
 * Whether a message is only emoji (up to 27, like Discord), so they're
 * drawn big. Server emoji count by their tokens.
 */
const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" });

export function onlyEmoji(content: string) {
  const rest = content.replace(EMOJI_TOKEN, " ").trim();
  // Most messages have a letter or digit in them, which no emoji is: no need to split them up.
  if (/[A-Za-z0-9]/.test(rest)) return false;
  const tokens = (content.match(EMOJI_TOKEN) ?? []).length;
  const segments = [...graphemes.segment(rest)].map((s) => s.segment).filter((s) => s.trim());
  if (!segments.every((s) => PICTOGRAPH.test(s) || /\p{Regional_Indicator}/u.test(s))) return false;
  const count = tokens + segments.length;
  return count > 0 && count <= 27;
}

/** Shrinks a picture to a 128px emoji (WebP); GIFs go up as they are, so they keep moving. */
export async function emojiPicture(file: File): Promise<Blob> {
  if (file.type === "image/gif") return file;
  const bitmap = await createImageBitmap(file);
  const scale = Math.min(1, 128 / Math.max(bitmap.width, bitmap.height));
  const width = Math.max(1, Math.round(bitmap.width * scale));
  const height = Math.max(1, Math.round(bitmap.height * scale));
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d")!;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(bitmap, 0, 0, width, height);
  bitmap.close();
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/webp", 0.92));
  return blob && blob.type === "image/webp" ? blob : await new Promise<Blob>((resolve) => canvas.toBlob((b) => resolve(b!), "image/png"));
}
