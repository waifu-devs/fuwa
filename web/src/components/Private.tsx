import { AnimatePresence, motion } from "motion/react";
import { useI18n } from "@/i18n/react";
import { usePrefs } from "@/lib/prefs";
import { HIDDEN_ADDRESS, hidesPersonal, maskName, shownAddress } from "@/lib/streamer";
import { cn } from "@/lib/utils";

/**
 * Something a stream shouldn't show: an instance's address, your own
 * username, or a secret like an invite code. In streamer mode it blurs away
 * into a stand-in instead of snapping, and blurs back when streamer mode ends.
 */
export function Private({ text, kind = "address", className }: { text: string; kind?: "address" | "name" | "secret"; className?: string }) {
  const { t } = useI18n();
  const hidden = usePrefs(hidesPersonal);
  const shown = hidden ? (kind === "name" ? maskName(text) : kind === "secret" ? "••••••••" : HIDDEN_ADDRESS) : text;
  return (
    <AnimatePresence mode="popLayout" initial={false}>
      <motion.span
        key={hidden ? "hidden" : "shown"}
        initial={{ opacity: 0, filter: "blur(6px)" }}
        animate={{ opacity: 1, filter: "blur(0px)" }}
        exit={{ opacity: 0, filter: "blur(8px)" }}
        transition={{ duration: 0.35, ease: "easeOut" }}
        title={hidden ? t("workspace.private.hidden") : undefined}
        className={cn("inline-block max-w-full truncate align-bottom", hidden && kind === "address" && "italic", className)}
      >
        {shown}
      </motion.span>
    </AnimatePresence>
  );
}

/** A class for inputs holding an address: blurred and dotted out while streamer mode hides it. */
export function usePrivateField() {
  return usePrefs(hidesPersonal) ? "private-field" : "";
}

/** An instance's address as it may be shown right now, following streamer mode. */
export const useAddress = (key: string) => usePrefs((p) => shownAddress(key, p));
