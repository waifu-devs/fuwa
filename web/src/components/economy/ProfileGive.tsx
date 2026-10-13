import { HandCoinsIcon } from "lucide-react";
import { m as motion } from "motion/react";
import { Permission, type Member, type User } from "@/gen/fuwa/v1/types_pb";
import { useEconomy } from "@/fuwa/economy";
import { useAccess } from "@/fuwa/hooks";
import { openMenuDialog } from "@/components/menus/dialogs";
import { useI18n } from "@/i18n/react";
import { has } from "@/lib/permissions";

/**
 * Under someone's profile card in a server with currencies: "Give currency",
 * for whoever has Grant Currency (docs/economy.md).
 */
export function ProfileGive({ instanceKey, user, member, onClose }: { instanceKey: string; user: User; member: Member | undefined; onClose: () => void }) {
  const { t } = useI18n();
  const serverId = member?.serverId ?? "";
  const access = useAccess(instanceKey, serverId);
  const { currencies } = useEconomy(instanceKey, serverId);
  if (!serverId || !currencies?.length || !has(access, Permission.GRANT_CURRENCY)) return null;
  return (
    <motion.div
      initial={{ opacity: 0, y: -6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", stiffness: 520, damping: 32, delay: 0.09 }}
      className="mt-2 flex rounded-2xl border bg-popover p-1.5 shadow-lg"
    >
      <button
        type="button"
        onClick={() => {
          onClose();
          openMenuDialog({ kind: "give", target: { instanceKey, serverId, user } });
        }}
        className="group flex flex-1 items-center justify-center gap-1.5 rounded-xl px-2 py-1.5 text-xs font-bold text-muted-foreground transition hover:bg-amber-500/10 hover:text-amber-600 active:scale-95 dark:hover:text-amber-400"
      >
        <HandCoinsIcon className="size-3.5 transition-transform duration-300 group-hover:-rotate-12" />
        {t("chattools.economy.giveItem")}
      </button>
    </motion.div>
  );
}
