import {
  autoUpdate,
  flip,
  FloatingFocusManager,
  FloatingPortal,
  offset,
  shift,
  useClick,
  useDismiss,
  useFloating,
  useInteractions,
  useRole,
} from "@floating-ui/react";
import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";
import { Permission } from "@/gen/fuwa/v1/types_pb";
import { run } from "@/fuwa/actions";
import { useAccess } from "@/fuwa/hooks";
import { providerName, sendGif, useGifSettings } from "@/fuwa/gifs";
import { lazyComponent } from "@/components/lazy";
import { SPRING } from "@/components/motion";
import { hasIn } from "@/lib/permissions";
import { cn } from "@/lib/utils";

const Picker = lazyComponent(() => import("./GifPanel").then((m) => m.GifPanel));

/**
 * The GIF button by the composer and the picker it opens: search (through
 * the instance), moods to browse, your saved GIFs and uploads, and what you
 * sent lately. Picking one sends it at once. Shown only where GIF search is
 * on, or you have saved GIFs to send.
 */
export function GifPicker({ instanceKey, serverId, channelId }: { instanceKey: string; serverId: string; channelId: string }) {
  const settings = useGifSettings(instanceKey);
  const access = useAccess(instanceKey, serverId);
  const [open, setOpen] = useState(false);
  const { refs, floatingStyles, context } = useFloating({
    open,
    onOpenChange: setOpen,
    placement: "top-end",
    whileElementsMounted: autoUpdate,
    middleware: [offset(8), flip(), shift({ padding: 8 })],
  });
  const { getReferenceProps, getFloatingProps } = useInteractions([useClick(context), useDismiss(context), useRole(context, { role: "dialog" })]);
  // GIFs are attachments: they need Attach Files where you are.
  if (!settings?.enabled || !hasIn(access, channelId, Permission.ATTACH_FILES)) return null;
  return (
    <>
      <motion.button
        ref={refs.setReference}
        {...getReferenceProps()}
        type="button"
        aria-label="GIFs"
        whileHover={{ scale: 1.1, rotate: 6 }}
        whileTap={{ scale: 0.85 }}
        className={cn(
          "mb-0.5 grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition-colors hover:text-primary",
          open && "bg-primary/10 text-primary",
        )}
      >
        <span className="rounded-md border-2 border-current px-1 text-[0.6rem] leading-[0.95rem] font-black tracking-wide">GIF</span>
      </motion.button>
      <FloatingPortal>
        <AnimatePresence>
          {open && (
            <FloatingFocusManager context={context} initialFocus={0} modal={false}>
              <div ref={refs.setFloating} style={floatingStyles} className="z-50" {...getFloatingProps()}>
                <motion.div
                  initial={{ opacity: 0, scale: 0.92, y: 10 }}
                  animate={{ opacity: 1, scale: 1, y: 0 }}
                  exit={{ opacity: 0, scale: 0.95, y: 8 }}
                  transition={SPRING}
                  style={{ transformOrigin: "bottom right" }}
                  className="flex h-[min(28rem,70vh)] w-[24rem] max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-2xl border bg-popover shadow-2xl"
                >
                  <Picker
                    instanceKey={instanceKey}
                    credit={providerName(settings.provider)}
                    onSend={async (gif) => {
                      await run(sendGif(instanceKey, serverId, channelId, gif));
                      setOpen(false);
                    }}
                  />
                </motion.div>
              </div>
            </FloatingFocusManager>
          )}
        </AnimatePresence>
      </FloatingPortal>
    </>
  );
}
