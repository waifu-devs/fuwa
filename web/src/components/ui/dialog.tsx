import * as DialogPrimitive from "@radix-ui/react-dialog";
import { AnimatePresence, motion } from "motion/react";
import { XIcon } from "lucide-react";
import { createContext, useContext, type ReactNode } from "react";
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

/**
 * A dialog that springs in from slightly below and fades the page behind it.
 * Controlled only: pass `open` and `onOpenChange`, so AnimatePresence can play
 * the exit before Radix unmounts it.
 */

const OpenContext = createContext(false);

export function Dialog({
  open,
  onOpenChange,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children: ReactNode;
}) {
  return (
    <DialogPrimitive.Root open={open} onOpenChange={onOpenChange}>
      <OpenContext.Provider value={open}>{children}</OpenContext.Provider>
    </DialogPrimitive.Root>
  );
}

export const DialogTrigger = DialogPrimitive.Trigger;
export const DialogClose = DialogPrimitive.Close;

export function DialogContent({
  className,
  children,
  wide = false,
}: {
  className?: string;
  children: ReactNode;
  wide?: boolean;
}) {
  const open = useContext(OpenContext);
  const { t } = useI18n();
  return (
    <AnimatePresence>
      {open && (
        <DialogPrimitive.Portal forceMount>
          <DialogPrimitive.Overlay asChild forceMount>
            <motion.div
              className="fixed inset-0 z-50 bg-black/50 backdrop-blur-[2px]"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.2 }}
            />
          </DialogPrimitive.Overlay>
          <div className="pointer-events-none fixed inset-0 z-50 grid place-items-end sm:place-items-center sm:p-4">
            <DialogPrimitive.Content asChild forceMount>
              <motion.div
                className={cn(
                  "pointer-events-auto relative max-h-[92svh] w-full overflow-y-auto rounded-t-3xl border bg-card p-6 text-card-foreground shadow-2xl outline-none sm:rounded-3xl",
                  wide ? "sm:max-w-2xl" : "sm:max-w-md",
                  className,
                )}
                initial={{ opacity: 0, y: 40, scale: 0.96 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, y: 24, scale: 0.97 }}
                transition={{ type: "spring", stiffness: 420, damping: 32 }}
              >
                {children}
                <DialogPrimitive.Close className="absolute top-4 right-4 grid size-8 place-items-center rounded-full text-muted-foreground transition hover:rotate-90 hover:bg-muted hover:text-foreground">
                  <XIcon className="size-4" />
                  <span className="sr-only">{t("common.close")}</span>
                </DialogPrimitive.Close>
              </motion.div>
            </DialogPrimitive.Content>
          </div>
        </DialogPrimitive.Portal>
      )}
    </AnimatePresence>
  );
}

export function DialogHeader({ title, description }: { title: ReactNode; description?: ReactNode }) {
  return (
    <div className="mb-5 pr-8">
      <DialogPrimitive.Title className="text-xl font-extrabold tracking-tight">{title}</DialogPrimitive.Title>
      {description ? (
        <DialogPrimitive.Description className="mt-1 text-sm text-muted-foreground">{description}</DialogPrimitive.Description>
      ) : (
        <DialogPrimitive.Description className="sr-only">{title}</DialogPrimitive.Description>
      )}
    </div>
  );
}
