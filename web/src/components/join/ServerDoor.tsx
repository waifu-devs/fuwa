import { BadgeCheckIcon, ClipboardPenIcon, HourglassIcon, ScrollTextIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { SPRING } from "@/components/motion";
import { useI18n } from "@/i18n/react";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";

/** What stands between someone and a server, as little chips: applications, rules, waifu.dev only, a minimum account age. */
export function ServerDoor({ server, className }: { server: Pick<Server, "applications" | "hasRules" | "linkedOnly" | "minAccountAgeSeconds">; className?: string }) {
  const { t } = useI18n();
  const chips = [
    server.applications && { id: "apply", icon: ClipboardPenIcon, label: t("join.apply") },
    server.hasRules && { id: "rules", icon: ScrollTextIcon, label: t("join.door.rules") },
    server.linkedOnly && { id: "linked", icon: BadgeCheckIcon, label: t("join.door.linkedOnly") },
    server.minAccountAgeSeconds > 0 && { id: "age", icon: HourglassIcon, label: t("join.door.accountAge", { age: formatDuration(server.minAccountAgeSeconds) }) },
  ].filter((c) => !!c);
  return (
    <div className={cn("flex flex-wrap gap-1.5 empty:hidden", className)}>
      <AnimatePresence initial={false} mode="popLayout">
        {chips.map((c, n) => (
          <motion.span
            key={c.id}
            layout
            initial={{ opacity: 0, scale: 0.6 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.6 }}
            transition={{ ...SPRING, delay: n * 0.04 }}
            className={cn(
              "flex items-center gap-1 rounded-full px-2 py-0.5 text-[0.7rem] font-bold",
              c.id === "apply" ? "bg-primary/15 text-primary" : "bg-muted text-muted-foreground",
            )}
          >
            <c.icon className="size-3" />
            {c.label}
          </motion.span>
        ))}
      </AnimatePresence>
    </div>
  );
}
