import { ShieldEllipsisIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { Permission } from "@/gen/fuwa/v1/types_pb";
import { useAccess } from "@/fuwa/hooks";
import { openModView, useOpenCases } from "@/fuwa/moderation";
import { useFuwa } from "@/fuwa/store";
import { Count } from "@/components/motion";
import { DropdownMenuItem } from "@/components/ui/dropdown-menu";
import { useI18n } from "@/i18n/react";
import { instanceHas } from "@/lib/compat";
import { has } from "@/lib/permissions";

/** Whether you may use Mod View in a server: the instance has it and you have Use Mod View. */
export function useModViewHere(instanceKey: string, serverId: string): boolean {
  const access = useAccess(instanceKey, serverId);
  const here = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "mod-view"));
  return here && has(access, Permission.MODERATE);
}

/** "Mod View" in the server's menu, with how many cases are open. */
export function ModViewItem({ instanceKey, server }: { instanceKey: string; server: Server | undefined }) {
  const { t } = useI18n();
  const shown = useModViewHere(instanceKey, server?.id ?? "") && !!server;
  const open = useOpenCases(instanceKey, server?.id ?? "", shown);
  if (!shown) return null;
  return (
    <DropdownMenuItem onSelect={() => openModView(instanceKey, server)}>
      <ShieldEllipsisIcon /> {t("chattools.modview.title")}
      <AnimatePresence initial={false}>
        {open > 0 && (
          <motion.span
            initial={{ scale: 0 }}
            animate={{ scale: 1 }}
            exit={{ scale: 0 }}
            transition={{ type: "spring", stiffness: 600, damping: 18 }}
            className="ml-auto grid h-5 min-w-5 place-items-center rounded-full bg-destructive px-1.5 text-[0.65rem] font-extrabold text-white"
          >
            <Count value={open} max={99} />
          </motion.span>
        )}
      </AnimatePresence>
    </DropdownMenuItem>
  );
}
