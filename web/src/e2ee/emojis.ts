/**
 * Server emoji in encrypted messages (direct messages and secure channels).
 * The instance can't read these messages, so it can't check or store the
 * emoji they use the way it does in a server's channels: the sender's device
 * carries each one it writes (its id, server, name and picture) inside the
 * encryption, and every device that opens it keeps only the ones it can draw
 * from the conversation's own instance (lib/emoji-search.ts). The instance
 * only serves the pictures, as it does for any emoji.
 */

import { create } from "@bufbuild/protobuf";
import { store } from "@/fuwa/store";
import { EmojiSchema, type Emoji } from "@/gen/fuwa/v1/types_pb";
import { carriedEmoji, emojisToCarry, keptEmojis, type CarriedEmoji } from "@/lib/emoji-search";

const originOf = (url: string | undefined) => {
  try {
    return url ? new URL(url).origin : "";
  } catch {
    return "";
  }
};

/** Where an instance's pictures are: its address, and the public one its links use. */
export function instanceOrigins(key: string): Set<string> {
  const inst = store.get().instances[key];
  const origins = new Set([originOf(inst?.url), originOf(inst?.node?.publicUrl)]);
  origins.delete("");
  return origins;
}

/** The emoji to send with a text: your servers' that it writes as tokens. */
export const emojisFor = (key: string, content: string): CarriedEmoji[] => emojisToCarry(store.get().instances[key]?.emojis, content);

/** Carried emoji as the protocol has them. */
export const toEmojis = (list: readonly CarriedEmoji[] | undefined): Emoji[] => (list ?? []).map((e) => create(EmojiSchema, e));

/** A text's carried emoji, checked; nothing for a text without any. */
export function emojisField(list: readonly CarriedEmoji[] | undefined, content: string, origins: ReadonlySet<string>): { emojis?: CarriedEmoji[] } {
  const emojis = keptEmojis(list, content, origins);
  return emojis.length ? { emojis } : {};
}

/** A reaction's server emoji, checked; undefined for a standard one or one that can't be drawn. */
export const customField = (custom: Emoji | undefined, origins: ReadonlySet<string>): CarriedEmoji | undefined =>
  carriedEmoji(custom, origins) ?? undefined;
