import { FlagIcon, ShieldEllipsisIcon } from "lucide-react";
import { m as motion } from "motion/react";
import type { Member, User } from "@/gen/fuwa/v1/types_pb";
import { byId } from "@/fuwa/hooks";
import { openModView } from "@/fuwa/moderation";
import { useFuwa } from "@/fuwa/store";
import { useModViewHere } from "@/components/modview/ModViewItem";
import { openMenuDialog } from "@/components/menus/dialogs";
import { useI18n } from "@/i18n/react";
import { instanceHas } from "@/lib/compat";

/**
 * Under someone's profile card in a server: "Report" for anyone looking at
 * someone else, and "Open in Mod View" for whoever may use it.
 */
export function ProfileReport({ instanceKey, user, member, me, onClose }: { instanceKey: string; user: User; member: Member | undefined; me: boolean; onClose: () => void }) {
  const { t } = useI18n();
  const serverId = member?.serverId ?? "";
  const here = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "mod-view"));
  const server = useFuwa((s) => byId(s.instances[instanceKey]?.servers, serverId));
  const moderates = useModViewHere(instanceKey, serverId);
  if (!here || !server) return null;
  const report = !me;
  if (!report && !moderates) return null;
  return (
    <motion.div
      initial={{ opacity: 0, y: -6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", stiffness: 520, damping: 32, delay: 0.1 }}
      className="mt-2 flex gap-1.5 rounded-2xl border bg-popover p-1.5 shadow-lg"
    >
      {moderates && (
        <button
          type="button"
          onClick={() => {
            onClose();
            openModView(instanceKey, server, { userId: user.id });
          }}
          className="group flex flex-1 items-center justify-center gap-1.5 rounded-xl px-2 py-1.5 text-xs font-bold text-muted-foreground transition hover:bg-primary/10 hover:text-primary active:scale-95"
        >
          <ShieldEllipsisIcon className="size-3.5 transition-transform duration-300 group-hover:scale-110" />
          {t("chattools.modview.openPerson")}
        </button>
      )}
      {report && (
        <button
          type="button"
          onClick={() => {
            onClose();
            openMenuDialog({ kind: "report", target: { kind: "member", instanceKey, serverId, user } });
          }}
          className="group flex flex-1 items-center justify-center gap-1.5 rounded-xl px-2 py-1.5 text-xs font-bold text-muted-foreground transition hover:bg-destructive/10 hover:text-destructive active:scale-95"
        >
          <FlagIcon className="size-3.5 transition-transform duration-300 group-hover:-rotate-12" />
          {t("chattools.report.memberItem")}
        </button>
      )}
    </motion.div>
  );
}
