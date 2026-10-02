import type { Emoji } from "@/gen/fuwa/v1/types_pb";
import { cn } from "@/lib/utils";

const TOKEN = /^<a?:([A-Za-z0-9_]{2,32}):([0-9A-Za-z]{10,32})>$/;

/**
 * One emoji written as text: a Unicode one as it is, or a server's own
 * (`<:name:id>`) as its picture. A server emoji that's gone shows its name.
 */
export function EmojiGlyph({ value, emojis, className }: { value: string; emojis: Emoji[] | undefined; className?: string }) {
  const token = TOKEN.exec(value.trim());
  if (!token) return <span className={cn("inline-grid place-items-center leading-none", className)}>{value}</span>;
  const emoji = emojis?.find((e) => e.id === token[2]);
  if (!emoji) return <span className={cn("text-xs text-muted-foreground", className)}>:{token[1]}:</span>;
  return <img src={emoji.url} alt={`:${emoji.name}:`} title={`:${emoji.name}:`} draggable={false} className={cn("inline-block object-contain", className)} />;
}
