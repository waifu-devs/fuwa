import { CrownIcon, ShieldIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useMemo, type CSSProperties } from "react";
import { MemberRole, type Member } from "@/gen/fuwa/v1/types_pb";
import { useInstance } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { displayName, hueOf, memberName } from "@/lib/format";

const EMPTY: Member[] = [];

const SECTIONS = [
  { role: MemberRole.OWNER, label: "Owner", icon: CrownIcon, tint: "text-amber-400" },
  { role: MemberRole.ADMIN, label: "Admins", icon: ShieldIcon, tint: "text-primary" },
  { role: MemberRole.MEMBER, label: "Members", icon: null, tint: "" },
] as const;

export function MemberList({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const inst = useInstance(instanceKey);
  const members = inst?.members[serverId] ?? EMPTY;
  const sections = useMemo(
    () =>
      SECTIONS.map((s) => ({
        ...s,
        members: members.filter((m) => (s.role === MemberRole.MEMBER ? m.role <= MemberRole.MEMBER : m.role === s.role)),
      })).filter((s) => s.members.length),
    [members],
  );
  return (
    <div className="scroll-thin h-full overflow-y-auto px-2 py-4">
      {sections.map((section) => (
        <section key={section.role} className="mb-4">
          <h3 className="mb-1 px-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">
            {section.label} — <Count value={section.members.length} />
          </h3>
          <ul>
            <AnimatePresence initial={false}>
              {section.members.map((m, n) => (
                <motion.li
                  key={m.user?.id}
                  layout
                  initial={{ opacity: 0, x: 16 }}
                  animate={{ opacity: 1, x: 0 }}
                  exit={{ opacity: 0, x: 16 }}
                  transition={{ type: "spring", stiffness: 500, damping: 36, delay: Math.min(n, 12) * 0.015 }}
                  className="group flex items-center gap-2.5 rounded-lg px-2 py-1.5 transition hover:bg-muted/70"
                >
                  <UserAvatar user={m.user} className="size-8 transition group-hover:scale-105" />
                  <span className="min-w-0">
                    <span className="flex items-center gap-1">
                      <span className="name-tint truncate text-sm font-bold" style={{ "--h": hueOf(m.user?.id ?? "") } as CSSProperties}>
                        {memberName(m)}
                      </span>
                      {section.icon && <section.icon className={`size-3 shrink-0 ${section.tint}`} />}
                    </span>
                    <span className="block truncate text-xs text-muted-foreground">
                      @{m.user?.username}
                      {m.nickname && m.nickname !== displayName(m.user) ? ` · ${displayName(m.user)}` : ""}
                    </span>
                  </span>
                </motion.li>
              ))}
            </AnimatePresence>
          </ul>
        </section>
      ))}
    </div>
  );
}
