import { StarIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";
import type { MessageGif } from "@/gen/fuwa/v1/types_pb";
import { run } from "@/fuwa/actions";
import { isSaved, providerName, saveGif, unsaveGif, useSavedGifs } from "@/fuwa/gifs";
import { GifImage } from "@/components/chat/GifImage";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const MAX_W = 320;
const MAX_H = 260;

/** How big a GIF shows: its own size, shrunk to fit, never stretched. */
export function fitGif(width: number, height: number, maxW = MAX_W, maxH = MAX_H) {
  const w = Math.max(1, width);
  const h = Math.max(1, height);
  const scale = Math.min(1, maxW / w, maxH / h);
  return { width: Math.max(48, Math.round(w * scale)), height: Math.max(32, Math.round(h * scale)) };
}

/**
 * A GIF in a message: sized before it loads (so the list doesn't jump),
 * with a star to save it on hover. With reduce motion it stays still until
 * it's pointed at or tapped.
 */
export function GifMessage({ gif, instanceKey, animate }: { gif: MessageGif | undefined; instanceKey: string; animate: boolean }) {
  const still = usePrefs(reduceMotion);
  const [hover, setHover] = useState(false);
  const [tapped, setTapped] = useState(false);
  const saved = useSavedGifs(instanceKey);
  if (!gif?.url) return null;
  const size = fitGif(gif.width, gif.height);
  const starred = isSaved(saved.list, gif.url);
  const playing = !still || hover || tapped;
  const credit = providerName(gif.provider);

  function toggle() {
    run(starred ? unsaveGif(instanceKey, gif!.url) : saveGif(instanceKey, gif!.url)).then(
      () => toast(starred ? "Removed from your GIFs" : "Saved to your GIFs"),
      (err: Error) => toast(err.message),
    );
  }

  return (
    <motion.div
      initial={animate ? { opacity: 0, scale: 0.94, y: 6 } : false}
      animate={{ opacity: 1, scale: 1, y: 0 }}
      transition={{ type: "spring", stiffness: 460, damping: 30 }}
      style={{ width: size.width, height: size.height }}
      onPointerEnter={() => setHover(true)}
      onPointerLeave={() => setHover(false)}
      className="group/gif relative mt-1 max-w-full overflow-hidden rounded-xl bg-muted shadow-sm"
    >
      <button
        type="button"
        onClick={() => still && setTapped((t) => !t)}
        onFocus={() => setHover(true)}
        onBlur={() => setHover(false)}
        aria-label={gif.title || "GIF"}
        className={cn("block size-full", still ? "cursor-pointer" : "cursor-default")}
      >
        <GifImage src={gif.url} playing={playing} alt={gif.title || "GIF"} className="size-full object-cover" />
      </button>
      <AnimatePresence>
        {still && !playing && (
          <motion.span
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="pointer-events-none absolute bottom-2 left-2 rounded-md bg-black/60 px-1.5 py-0.5 text-[0.65rem] font-extrabold tracking-wide text-white"
          >
            GIF
          </motion.span>
        )}
      </AnimatePresence>
      <button
        type="button"
        onClick={toggle}
        aria-label={starred ? "Remove from your GIFs" : "Save to your GIFs"}
        aria-pressed={starred}
        className={cn(
          "absolute top-1.5 right-1.5 grid size-8 place-items-center rounded-lg bg-black/55 text-white opacity-0 transition-[opacity,transform] duration-150 group-hover/gif:opacity-100 focus-visible:opacity-100 active:scale-90",
          starred && "opacity-100",
        )}
      >
        <StarIcon className={cn("size-4", starred && "fill-amber-400 text-amber-400")} />
      </button>
      {credit && (
        <span className="pointer-events-none absolute right-2 bottom-1.5 text-[0.6rem] font-bold text-white/85 opacity-0 transition-opacity duration-150 [text-shadow:0_1px_2px_rgb(0_0_0/0.6)] group-hover/gif:opacity-100">
          via {credit}
        </span>
      )}
    </motion.div>
  );
}
