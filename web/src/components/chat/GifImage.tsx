import { useEffect, useRef } from "react";
import { shownPicture } from "@/lib/shown";
import { cn } from "@/lib/utils";

/**
 * A GIF that moves only while `playing`; otherwise its still (a still frame
 * the instance gave, or the first frame drawn here). Reduce motion keeps
 * GIFs still until someone hovers, focuses or taps them. Pictures load only
 * from fuwa instances.
 */
export function GifImage({
  src,
  still,
  playing,
  alt,
  className,
}: {
  src: string;
  still?: string;
  playing: boolean;
  alt: string;
  className?: string;
}) {
  const moving = shownPicture(src);
  const frame = shownPicture(still);
  if (!moving) return <span className={cn("block bg-muted", className)} />;
  if (playing) return <img src={moving} alt={alt} draggable={false} decoding="async" className={className} />;
  if (frame) return <img src={frame} alt={alt} draggable={false} decoding="async" className={className} />;
  return <FirstFrame src={moving} alt={alt} className={className} />;
}

/** The first frame of the GIF at `src`, drawn once on a canvas. */
function FirstFrame({ src, alt, className }: { src: string; alt: string; className?: string }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const img = new Image();
    img.decoding = "async";
    img.onload = () => {
      const c = canvas.current;
      if (!c) return;
      c.width = img.naturalWidth;
      c.height = img.naturalHeight;
      c.getContext("2d")?.drawImage(img, 0, 0);
    };
    img.src = src;
    return () => {
      img.onload = null;
    };
  }, [src]);
  return <canvas ref={canvas} role="img" aria-label={alt} className={className} />;
}
