import { BotIcon, MessageSquareWarningIcon, UserRoundXIcon, type LucideIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { createContext, useContext } from "react";
import type { Server, User } from "@/gen/fuwa/v1/types_pb";
import { Private } from "@/components/Private";
import { useI18n } from "@/i18n/react";
import { AUTO_MOD, DISMISSED, MEMBER_REPORT, reasonLabel, RESOLVED, statusLabel } from "@/lib/mod-view";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";

/** What every part of Mod View shares: where it is, who's been seen, and how to go elsewhere in it. */
export type ModViewScope = {
  instanceKey: string;
  server: Server;
  meId: string;
  /** People named by what's been loaded (subjects, reporters, moderators, note authors). */
  user: (id: string) => User | undefined;
  learn: (users: readonly User[]) => void;
  /** Someone's page. */
  openPerson: (userId: string) => void;
  /** The queue, every state, for one person. */
  showCases: (userId: string) => void;
  /** One case, in the queue. */
  openCase: (caseId: string) => void;
};

export const ModViewContext = createContext<ModViewScope | null>(null);

export function useScope(): ModViewScope {
  const scope = useContext(ModViewContext);
  if (!scope) throw new Error("outside Mod View");
  return scope;
}

/** A case's kind as an icon and tint. */
export const KIND_LOOK: Record<number, { icon: LucideIcon; tint: string }> = {
  [AUTO_MOD]: { icon: BotIcon, tint: "bg-violet-500/15 text-violet-500" },
  [MEMBER_REPORT]: { icon: UserRoundXIcon, tint: "bg-orange-500/15 text-orange-500" },
};
export const kindLook = (kind: number) => KIND_LOOK[kind] ?? { icon: MessageSquareWarningIcon, tint: "bg-destructive/15 text-destructive" };

/** A case's state as a pill, swapping when it changes. */
export function StatusPill({ status, className }: { status: number; className?: string }) {
  const { t } = useI18n();
  const tint =
    status === RESOLVED ? "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400" : status === DISMISSED ? "bg-muted text-muted-foreground" : "bg-amber-500/15 text-amber-600 dark:text-amber-400";
  return (
    <AnimatePresence mode="popLayout" initial={false}>
      <motion.span
        key={status}
        initial={{ scale: 0.6, opacity: 0 }}
        animate={{ scale: 1, opacity: 1 }}
        exit={{ scale: 0.6, opacity: 0 }}
        transition={SPRING}
        className={cn("inline-flex shrink-0 items-center rounded-full px-2 py-0.5 text-[0.7rem] font-extrabold", tint, className)}
      >
        {t(statusLabel(status))}
      </motion.span>
    </AnimatePresence>
  );
}

/** The reasons people gave, as chips. */
export function ReasonChips({ reasons, className }: { reasons: readonly number[]; className?: string }) {
  const { t } = useI18n();
  const unique = [...new Set(reasons)];
  if (!unique.length) return null;
  return (
    <span className={cn("flex flex-wrap gap-1", className)}>
      {unique.map((r) => (
        <span key={r} className="rounded-full bg-destructive/10 px-1.5 py-px text-[0.65rem] font-bold text-destructive">
          {t(reasonLabel(r))}
        </span>
      ))}
    </span>
  );
}

/** Someone's @username; your own hides in streamer mode. */
export function Handle({ user, me }: { user: User | undefined; me: boolean }) {
  if (!user?.username) return null;
  return <span className="truncate">@{me ? <Private text={user.username} kind="name" /> : user.username}</span>;
}

/** A section's heading inside a page. */
export function Heading({ icon: Icon, children, aside }: { icon: LucideIcon; children: React.ReactNode; aside?: React.ReactNode }) {
  return (
    <h3 className="mb-2 flex items-center gap-2 text-xs font-extrabold tracking-wide text-muted-foreground uppercase">
      <Icon className="size-3.5" />
      <span className="flex-1">{children}</span>
      {aside}
    </h3>
  );
}

/** Nothing to show: an icon, a line, and a softer one under it. */
export function Empty({ icon: Icon, title, hint }: { icon: LucideIcon; title: string; hint?: string }) {
  return (
    <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col items-center gap-2 py-10 text-center">
      <motion.span
        initial={{ scale: 0.6, rotate: -12 }}
        animate={{ scale: 1, rotate: 0 }}
        transition={{ type: "spring", stiffness: 420, damping: 14 }}
        className="grid size-12 place-items-center rounded-2xl bg-muted text-muted-foreground"
      >
        <Icon className="size-6" />
      </motion.span>
      <p className="font-extrabold">{title}</p>
      {hint && <p className="max-w-xs text-sm text-muted-foreground">{hint}</p>}
    </motion.div>
  );
}

/** Rows while a list loads. */
export function Shimmer({ rows = 4, className }: { rows?: number; className?: string }) {
  return (
    <div className="flex flex-col gap-2">
      {Array.from({ length: rows }, (_, n) => (
        <div key={n} className={cn("shimmer h-16 rounded-2xl", className)} />
      ))}
    </div>
  );
}
