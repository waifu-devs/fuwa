import { CrownIcon, HourglassIcon, ShieldIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useMemo, type CSSProperties } from "react";
import { MemberRole, type Member } from "@/gen/fuwa/v1/types_pb";
import { useInstance } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { CopyId } from "@/components/CopyId";
import { Count } from "@/components/motion";
import { Private } from "@/components/Private";
import { InlineMarkdown } from "@/components/Markdown";
import { ProfilePopover } from "@/components/ProfilePopover";
import { displayName, formatStamp, hueOf, memberName, shownStatus, timedOutUntil } from "@/lib/format";
import { useNow } from "@/lib/notifications";

const EMPTY: Member[] = [];

const SECTIONS = [
  { role: MemberRole.OWNER, label: "Owner", icon: CrownIcon, tint: "text-amber-400" },
  { role: MemberRole.ADMIN, label: "Admins", icon: ShieldIcon, tint: "text-primary" },
  { role: MemberRole.MEMBER, label: "Members", icon: null, tint: "" },
] as const;

export function MemberList({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const inst = useInstance(instanceKey);
  const now = useNow(60_000);
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
                  className="row-y group flex items-center gap-1 rounded-lg px-2 transition hover:bg-muted/70"
                >
                  <ProfilePopover instanceKey={instanceKey} user={m.user} member={m} side="left">
                    <button type="button" className="flex min-w-0 flex-1 items-center gap-2.5 text-left">
                      <UserAvatar user={m.user} className="size-8 transition duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:scale-105 group-active:scale-95" />
                      <span className="min-w-0 flex-1">
                        <span className="flex items-center gap-1">
                          <span className="name-tint truncate text-sm font-bold" style={{ "--h": hueOf(m.user?.id ?? "") } as CSSProperties}>
                            {memberName(m)}
                          </span>
                          {section.icon && <section.icon className={`size-3 shrink-0 ${section.tint}`} />}
                          <TimedOutMark member={m} now={now} />
                        </span>
                        <MemberSubtitle member={m} me={m.user?.id === inst?.me?.id} now={now} />
                      </span>
                    </button>
                  </ProfilePopover>
                  {m.user && (
                    <span className="opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
                      <CopyId id={m.user.id} what="user ID" />
                    </span>
                  )}
                </motion.li>
              ))}
            </AnimatePresence>
          </ul>
        </section>
      ))}
    </div>
  );
}

/** An hourglass by the names of members who are timed out. */
function TimedOutMark({ member, now }: { member: Member; now: number }) {
  const until = timedOutUntil(member, now);
  return (
    <AnimatePresence>
      {until && (
        <motion.span
          initial={{ scale: 0, rotate: -90 }}
          animate={{ scale: 1, rotate: 0 }}
          exit={{ scale: 0, rotate: 90 }}
          transition={{ type: "spring", stiffness: 600, damping: 18 }}
          title={`Timed out until ${formatStamp(until)}`}
          className="shrink-0 text-amber-500"
        >
          <HourglassIcon className="size-3" aria-label="Timed out" />
        </motion.span>
      )}
    </AnimatePresence>
  );
}

/** Under a member's name: their custom status, else their username. */
function MemberSubtitle({ member, me, now }: { member: Member; me: boolean; now: number }) {
  const status = shownStatus(member.user, now);
  return (
    <span className="relative block h-4 overflow-hidden text-xs text-muted-foreground">
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={status ? `s:${status}` : "username"}
          initial={{ y: 12, opacity: 0 }}
          animate={{ y: 0, opacity: 1 }}
          exit={{ y: -12, opacity: 0 }}
          transition={{ type: "spring", stiffness: 500, damping: 32 }}
          className="block truncate"
        >
          {status ? (
            <InlineMarkdown links={false}>{status}</InlineMarkdown>
          ) : (
            <>
              @{me ? <Private text={member.user?.username ?? ""} kind="name" /> : member.user?.username}
              {member.nickname && member.nickname !== displayName(member.user) ? ` · ${displayName(member.user)}` : ""}
            </>
          )}
        </motion.span>
      </AnimatePresence>
    </span>
  );
}
