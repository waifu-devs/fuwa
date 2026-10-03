import { BotIcon } from "lucide-react";
import { motion } from "motion/react";

/** Marks what isn't a person: an app posting through a webhook, or an agent (an account a program drives). */
export function AppBadge({ agent = false }: { agent?: boolean }) {
  return (
    <motion.span
      initial={{ scale: 0.6, opacity: 0 }}
      animate={{ scale: 1, opacity: 1 }}
      whileHover={{ scale: 1.08, rotate: -3 }}
      transition={{ type: "spring", stiffness: 600, damping: 18 }}
      title={agent ? "An agent: an account a program drives" : "Posted by an app through a webhook"}
      className="inline-flex shrink-0 items-center gap-0.5 rounded bg-primary/15 px-1 py-px text-[0.6rem] leading-none font-extrabold tracking-wide text-primary"
    >
      {agent && <BotIcon aria-hidden className="size-2.5" />}
      {agent ? "AGENT" : "APP"}
    </motion.span>
  );
}
