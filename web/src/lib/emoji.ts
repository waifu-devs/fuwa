/**
 * Emoji: a server's own (written `<:name:id>`, or `<a:name:id>` when it
 * moves) and a short list of everyday Unicode ones the pickers offer.
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

/** Everyday Unicode emoji, with the names people type after a colon. */
export const UNICODE_EMOJI: { char: string; names: string[] }[] = [
  ["😀", "grinning smile happy"],
  ["😄", "smile happy joy"],
  ["😂", "joy laugh lol tears"],
  ["🤣", "rofl rolling laugh"],
  ["😊", "blush smile"],
  ["😇", "innocent halo angel"],
  ["🙂", "slight_smile"],
  ["😉", "wink"],
  ["😍", "heart_eyes love"],
  ["🥰", "smiling_hearts love"],
  ["😘", "kiss"],
  ["😋", "yum tasty"],
  ["😛", "tongue"],
  ["😜", "wink_tongue crazy"],
  ["🤪", "zany crazy"],
  ["🤗", "hug hugs"],
  ["🤔", "thinking think hmm"],
  ["🤨", "raised_eyebrow sus"],
  ["😐", "neutral meh"],
  ["😑", "expressionless"],
  ["🙄", "eye_roll rolling_eyes"],
  ["😏", "smirk"],
  ["😴", "sleeping zzz"],
  ["😌", "relieved"],
  ["🥺", "pleading puppy please"],
  ["😢", "cry sad"],
  ["😭", "sob crying"],
  ["😤", "triumph huff"],
  ["😠", "angry mad"],
  ["🤯", "mind_blown exploding"],
  ["😳", "flushed blush"],
  ["🥵", "hot"],
  ["🥶", "cold freezing"],
  ["😱", "scream shock"],
  ["😅", "sweat_smile phew"],
  ["😬", "grimace yikes"],
  ["🫠", "melting"],
  ["🫡", "salute"],
  ["🤫", "shush quiet"],
  ["🤭", "giggle oops"],
  ["😎", "sunglasses cool"],
  ["🤓", "nerd"],
  ["🥳", "partying_face celebrate"],
  ["😈", "smiling_imp devil"],
  ["💀", "skull dead"],
  ["👻", "ghost boo"],
  ["🤖", "robot bot"],
  ["👀", "eyes look"],
  ["👍", "thumbsup +1 yes like"],
  ["👎", "thumbsdown -1 no"],
  ["👏", "clap applause"],
  ["🙌", "raised_hands hooray"],
  ["🙏", "pray please thanks"],
  ["👋", "wave hello hi bye"],
  ["🤝", "handshake deal"],
  ["✌️", "v peace"],
  ["🤞", "crossed_fingers luck"],
  ["👌", "ok_hand perfect"],
  ["🤙", "call_me shaka"],
  ["💪", "muscle strong flex"],
  ["🫶", "heart_hands"],
  ["❤️", "heart love red"],
  ["🩷", "pink_heart"],
  ["🧡", "orange_heart"],
  ["💛", "yellow_heart"],
  ["💚", "green_heart"],
  ["💙", "blue_heart"],
  ["💜", "purple_heart"],
  ["🖤", "black_heart"],
  ["🤍", "white_heart"],
  ["💔", "broken_heart"],
  ["💖", "sparkling_heart"],
  ["💕", "two_hearts"],
  ["✨", "sparkles shiny"],
  ["⭐", "star"],
  ["🌟", "glowing_star"],
  ["🔥", "fire lit hot"],
  ["💯", "100 hundred"],
  ["🎉", "tada party celebrate"],
  ["🎊", "confetti"],
  ["🎁", "gift present"],
  ["🎂", "cake birthday"],
  ["🏆", "trophy win"],
  ["🥇", "first_place gold"],
  ["🎮", "video_game gaming controller"],
  ["🎧", "headphones music"],
  ["🎵", "music note"],
  ["🎨", "art paint"],
  ["📚", "books study"],
  ["💻", "computer laptop code"],
  ["🛠️", "tools build"],
  ["🐛", "bug"],
  ["🚀", "rocket ship launch"],
  ["💡", "bulb idea"],
  ["📌", "pin pushpin"],
  ["📣", "mega announcement"],
  ["📝", "memo note"],
  ["✅", "white_check_mark check done yes"],
  ["❌", "x no cross"],
  ["⚠️", "warning"],
  ["❓", "question"],
  ["❗", "exclamation"],
  ["💤", "zzz sleep"],
  ["💬", "speech chat"],
  ["🔔", "bell"],
  ["🔒", "lock"],
  ["🌸", "cherry_blossom sakura flower"],
  ["🌈", "rainbow"],
  ["☀️", "sun sunny"],
  ["🌙", "moon night"],
  ["⚡", "zap lightning"],
  ["☕", "coffee"],
  ["🍵", "tea"],
  ["🍕", "pizza"],
  ["🍜", "ramen noodles"],
  ["🍣", "sushi"],
  ["🍙", "rice_ball onigiri"],
  ["🍰", "shortcake"],
  ["🍓", "strawberry"],
  ["🐱", "cat"],
  ["🐶", "dog"],
  ["🦊", "fox"],
  ["🐰", "rabbit bunny"],
  ["🐻", "bear"],
  ["🐼", "panda"],
  ["🐸", "frog"],
  ["🐧", "penguin"],
  ["🦄", "unicorn"],
  ["🐉", "dragon"],
  ["🍀", "four_leaf_clover luck"],
  ["🌊", "wave ocean"],
  ["🏠", "house home"],
  ["👑", "crown king queen"],
  ["💎", "gem diamond"],
].map(([char, names]) => ({ char: char!, names: names!.split(" ") }));

/** One emoji to pick: a server's own (with its picture) or a Unicode one. */
export type EmojiChoice = { key: string; name: string; text: string; url?: string; char?: string };

/** Server emoji and Unicode ones whose names start with (then contain) `query`. */
export function searchEmoji(query: string, server: Emoji[], limit = 8): EmojiChoice[] {
  const q = query.toLowerCase();
  const own = server
    .filter((e) => e.name.toLowerCase().includes(q))
    .map((e) => ({ key: e.id, name: e.name, text: emojiToken(e), url: e.url, rank: e.name.toLowerCase().startsWith(q) ? 0 : 2 }));
  const plain = UNICODE_EMOJI.flatMap((e) => {
    const name = e.names.find((n) => n.startsWith(q)) ?? e.names.find((n) => n.includes(q));
    return name ? [{ key: e.char, name, text: e.char, char: e.char, rank: name.startsWith(q) ? 1 : 3 }] : [];
  });
  return [...own, ...plain]
    .sort((a, b) => a.rank - b.rank)
    .slice(0, limit)
    .map(({ rank: _, ...choice }) => choice);
}

const PICTOGRAPH = /\p{Extended_Pictographic}/u;

/**
 * Whether a message is only emoji (up to 27, like Discord), so they're
 * drawn big. Server emoji count by their tokens.
 */
export function onlyEmoji(content: string) {
  const rest = content.replace(EMOJI_TOKEN, " ").trim();
  const tokens = (content.match(EMOJI_TOKEN) ?? []).length;
  const segments = [...new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(rest)].map((s) => s.segment).filter((s) => s.trim());
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
