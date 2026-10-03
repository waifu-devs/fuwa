import { CrownIcon, HourglassIcon } from "lucide-react";
import { AnimatePresence, LayoutGroup, motion } from "motion/react";
import { useMemo } from "react";
import type { Member, Role } from "@/gen/fuwa/v1/types_pb";
import { useInstance, useRoles } from "@/fuwa/hooks";
import { RoleDot } from "@/components/chat/mentions";
import { RoleName } from "@/components/RoleName";
import { UserAvatar } from "@/components/Icons";
import { CopyId } from "@/components/CopyId";
import { Count } from "@/components/motion";
import { Private } from "@/components/Private";
import { InlineMarkdown } from "@/components/Markdown";
import { ProfilePopover } from "@/components/ProfilePopover";
import { AppBadge } from "@/components/AppBadge";
import { displayName, formatStamp, isAgent, memberName, shownStatus, timedOutUntil } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { colorOf, hoistedRole } from "@/lib/permissions";

const EMPTY: Member[] = [];

type Section = { id: string; role: Role | null; members: Member[] };

/**
 * Everyone in the server, under their highest role that's shown apart
 * (hoisted), then everyone else. Someone given or losing a role glides to
 * their new place.
 */
export function MemberList({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const inst = useInstance(instanceKey);
  const now = useNow(60_000);
  const members = inst?.members[serverId] ?? EMPTY;
  const roles = useRoles(instanceKey, serverId);
  const ownerId = inst?.servers.find((s) => s.id === serverId)?.ownerId;
  const sections = useMemo(() => {
    const byRole = new Map<string, Member[]>();
    const rest: Member[] = [];
    for (const m of members) {
      const role = hoistedRole(roles, m);
      if (role) byRole.set(role.id, [...(byRole.get(role.id) ?? []), m]);
      else rest.push(m);
    }
    const out: Section[] = roles.filter((r) => byRole.has(r.id)).map((r) => ({ id: r.id, role: r, members: byRole.get(r.id)! }));
    if (rest.length) out.push({ id: "members", role: null, members: rest });
    return out;
  }, [members, roles]);
  return (
    <div className="scroll-thin h-full overflow-y-auto px-2 py-4">
      <LayoutGroup id={`members-${serverId}`}>
      <AnimatePresence initial={false}>
      {sections.map((section) => (
        <motion.section
          key={section.id}
          layout="position"
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, height: 0 }}
          transition={{ type: "spring", stiffness: 500, damping: 36 }}
          className="mb-4"
        >
          <h3 className="mb-1 flex items-center gap-1.5 px-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">
            {section.role && <RoleDot role={section.role} className="size-2" />}
            <span className="truncate">{section.role?.name ?? "Members"}</span> — <Count value={section.members.length} />
          </h3>
          <ul>
            <AnimatePresence initial={false}>
              {section.members.map((m, n) => (
                <motion.li
                  key={m.user?.id}
                  layoutId={`member-${serverId}-${m.user?.id}`}
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
                          <RoleName id={m.user?.id ?? ""} name={memberName(m)} color={colorOf(roles, m)} className="text-sm" />
                          {m.user?.id === ownerId && <CrownIcon aria-label="Owner" className="size-3 shrink-0 text-amber-400" />}
                          {isAgent(m.user) && <AppBadge agent />}
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
        </motion.section>
      ))}
      </AnimatePresence>
      </LayoutGroup>
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
