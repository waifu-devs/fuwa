import { useEffect, useRef, useState, type HTMLAttributes } from "react";
import type { Emoji } from "@/gen/fuwa/v1/types_pb";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { cn } from "@/lib/utils";

/**
 * A server emoji's picture. Moving ones play, except with Reduce motion on:
 * then they hold their first frame until the pointer is over them (or
 * `playing` says to, as the picker does for the one it's on).
 */
export function EmojiImage({
  emoji,
  playing = false,
  className,
  title = true,
}: {
  emoji: Pick<Emoji, "url" | "name" | "animated">;
  playing?: boolean;
  className?: string;
  title?: boolean;
}) {
  const calm = usePrefs(reduceMotion);
  const [hover, setHover] = useState(false);
  const [broken, setBroken] = useState(false);
  const label = `:${emoji.name}:`;
  if (broken) return <span className={cn("text-xs text-muted-foreground", className)}>{label}</span>;
  const still = calm && emoji.animated && !playing && !hover;
  const shared = {
    title: title ? label : undefined,
    onPointerEnter: () => setHover(true),
    onPointerLeave: () => setHover(false),
    draggable: false,
    className: cn("inline-block object-contain", className),
  } as const;
  if (still) return <FirstFrame src={emoji.url} onBroken={setBroken} role="img" aria-label={label} {...shared} />;
  return <img src={emoji.url} alt={label} onError={() => setBroken(true)} {...shared} />;
}

/** A moving picture's first frame, drawn once. */
function FirstFrame({ src, onBroken, ...props }: { src: string; onBroken: (broken: boolean) => void } & HTMLAttributes<HTMLCanvasElement>) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const picture = new Image();
    let live = true;
    picture.decoding = "async";
    picture.onload = () => {
      const el = canvas.current;
      if (!live || !el) return;
      el.width = picture.naturalWidth;
      el.height = picture.naturalHeight;
      el.getContext("2d")?.drawImage(picture, 0, 0);
    };
    picture.onerror = () => live && onBroken(true);
    picture.src = src;
    return () => {
      live = false;
    };
  }, [src, onBroken]);
  return <canvas ref={canvas} {...props} />;
}
