import { FingerprintIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { SPRING } from "@/components/motion";
import { Toggle } from "@/components/settings/controls";
import { setPrefs, usePrefs } from "@/lib/prefs";
import { PrefSetting } from "./common";

export function Advanced() {
  const on = usePrefs((p) => p.developerMode);
  return (
    <div className="flex flex-col">
      <PrefSetting id="developer-mode" title="Developer mode" keys={["developerMode"]}>
        <Toggle
          checked={on}
          onChange={(developerMode) => setPrefs({ developerMode })}
          label="Show Copy ID on servers, channels, people and messages"
          hint="Handy for bots, the API and bug reports."
        />
        <div className="flex items-center gap-2 rounded-xl bg-muted/50 px-3 py-2.5 text-sm">
          <span className="min-w-0 flex-1 truncate">
            <b>#general</b> <span className="text-muted-foreground">· the channel's header</span>
          </span>
          <AnimatePresence initial={false}>
            {on && (
              <motion.span
                initial={{ opacity: 0, scale: 0.6, x: 8 }}
                animate={{ opacity: 1, scale: 1, x: 0 }}
                exit={{ opacity: 0, scale: 0.6, x: 8 }}
                transition={SPRING}
                className="flex shrink-0 items-center gap-1.5 rounded-lg bg-background px-2 py-1 text-xs font-bold shadow-sm"
              >
                <FingerprintIcon className="size-3.5 text-primary" /> Copy channel ID
              </motion.span>
            )}
          </AnimatePresence>
        </div>
      </PrefSetting>
    </div>
  );
}
