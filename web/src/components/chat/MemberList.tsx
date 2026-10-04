import { CrownIcon, HourglassIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Member, Role } from "@/gen/fuwa/v1/types_pb";
import type { Activity } from "@/gen/fuwa/v1/presence_pb";
import { useRoles } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { RoleDot } from "@/components/chat/mentions";
import { RoleName } from "@/components/RoleName";
import { UserAvatar } from "@/components/Icons";
import { CopyId } from "@/components/CopyId";
import { Count } from "@/components/motion";
import { Private } from "@/components/Private";
import { InlineMarkdown } from "@/components/Markdown";
import { ProfilePopover } from "@/components/ProfilePopover";
import { AppBadge } from "@/components/AppBadge";
import { ActivityLine, PresenceDot } from "@/components/Presence";
import { useOnline, usePresence } from "@/fuwa/presence";
import { displayName, formatStamp, isAgent, memberName, shownStatus, timedOutUntil } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { colorOf, hoistedRole } from "@/lib/permissions";

const EMPTY: Member[] = [];

type Section = { id: string; role: Role | null; label?: string; members: Member[] };

/** One line of the list: a section's heading or a member, at its place from the top. */
type Item =
  | { kind: "heading"; key: string; top: number; section: Section }
  | { kind: "member"; key: string; top: number; member: Member };

/** Lines drawn past the edges, so a quick scroll doesn't show blank space. */
const OVERSCAN = 8;
/** Space between sections (the old `mb-4`). */
const SECTION_GAP = 16;

/**
 * Everyone in the server, under their highest role that's shown apart
 * (hoisted), then everyone else. Where the instance keeps presence, those
 * are the people online, and everyone offline comes last, faded. Someone
 * given or losing a role, or coming online, glides to their new place.
 *
 * Servers can have thousands of members, so only the lines in view are
 * drawn: every line has the same height (measured from the first one drawn,
 * so it follows the density setting), and each sits at its place with a
 * transform that eases when the place changes.
 */
export function MemberList({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const now = useNow(60_000);
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? EMPTY);
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id);
  const ownerId = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.ownerId);
  const roles = useRoles(instanceKey, serverId);
  // Who's online changes at most a few times a second, as one new Set.
  const online = useOnline(instanceKey);
  const sections = useMemo(() => {
    const byRole = new Map<string, Member[]>();
    const rest: Member[] = [];
    const offline: Member[] = [];
    for (const m of members) {
      if (online && !online.has(m.user?.id ?? "")) {
        offline.push(m);
        continue;
      }
      const role = hoistedRole(roles, m);
      if (role) {
        const list = byRole.get(role.id);
        if (list) list.push(m);
        else byRole.set(role.id, [m]);
      } else rest.push(m);
    }
    const out: Section[] = roles.filter((r) => byRole.has(r.id)).map((r) => ({ id: r.id, role: r, members: byRole.get(r.id)! }));
    if (rest.length) out.push({ id: "members", role: null, label: online ? "Online" : "Members", members: rest });
    if (offline.length) out.push({ id: "offline", role: null, label: "Offline", members: offline });
    return out;
  }, [members, roles, online]);

  // Line heights, measured once drawn (and again if the density or text size changes).
  const [heights, setHeights] = useState({ heading: 20, member: 48 });
  const measure = useCallback((kind: "heading" | "member", el: HTMLElement | null) => {
    if (!el) return;
    const h = el.offsetHeight;
    if (h > 0) setHeights((cur) => (cur[kind] === h ? cur : { ...cur, [kind]: h }));
  }, []);

  const { items, total } = useMemo(() => {
    const out: Item[] = [];
    let top = 0;
    sections.forEach((section, n) => {
      if (n > 0) top += SECTION_GAP;
      out.push({ kind: "heading", key: `heading-${section.id}`, top, section });
      top += heights.heading + 4;
      for (const m of section.members) {
        out.push({ kind: "member", key: `member-${m.user?.id}`, top, member: m });
        top += heights.member;
      }
    });
    return { items: out, total: top };
  }, [sections, heights]);

  // The part of the list in view.
  const scroller = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: 800 });
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const update = () => setView((v) => (v.top === el.scrollTop && v.height === el.clientHeight ? v : { top: el.scrollTop, height: el.clientHeight }));
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const onScroll = useCallback(() => {
    const el = scroller.current;
    if (el) setView({ top: el.scrollTop, height: el.clientHeight });
  }, []);
  const from = view.top - OVERSCAN * heights.member;
  const to = view.top + view.height + OVERSCAN * heights.member;
  const shown = items.filter((item) => item.top + heights.member >= from && item.top <= to);

  // Only people who just joined slide in; lines scrolled into view just appear.
  const known = useRef<Set<string> | null>(null);
  const joined = useMemo(() => {
    const ids = new Set(members.map((m) => m.user?.id ?? ""));
    const fresh = known.current ? new Set([...ids].filter((id) => !known.current!.has(id))) : new Set<string>();
    return { ids, fresh };
  }, [members]);
  useEffect(() => {
    known.current = joined.ids;
  }, [joined]);

  let measuredHeading = false;
  let measuredMember = false;
  return (
    <div ref={scroller} onScroll={onScroll} className="scroll-thin h-full overflow-y-auto px-2 py-4">
      <div className="relative" style={{ height: total }}>
        <AnimatePresence initial={false} custom={joined.ids} presenceAffectsLayout={false}>
          {shown.map((item) => {
            const first = item.kind === "heading" ? !measuredHeading && (measuredHeading = true) : !measuredMember && (measuredMember = true);
            return (
              <div
                key={item.key}
                ref={first ? (el) => measure(item.kind, el) : undefined}
                className="member-line absolute inset-x-0 top-0"
                style={{ transform: `translateY(${item.top}px)` }}
              >
                {item.kind === "heading" ? (
                  <SectionHeading section={item.section} />
                ) : (
                  <MemberRow
                    instanceKey={instanceKey}
                    member={item.member}
                    roles={roles}
                    owner={item.member.user?.id === ownerId}
                    me={item.member.user?.id === meId}
                    now={now}
                    enter={joined.fresh.has(item.member.user?.id ?? "")}
                  />
                )}
              </div>
            );
          })}
        </AnimatePresence>
      </div>
    </div>
  );
}

function SectionHeading({ section }: { section: Section }) {
  return (
    <h3 className="flex items-center gap-1.5 px-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">
      {section.role && <RoleDot role={section.role} className="size-2" />}
      <span className="truncate">{section.role?.name ?? section.label ?? "Members"}</span> — <Count value={section.members.length} />
    </h3>
  );
}

const MemberRow = memo(function MemberRow({
  instanceKey,
  member: m,
  roles,
  owner,
  me,
  now,
  enter,
}: {
  instanceKey: string;
  member: Member;
  roles: Role[];
  owner: boolean;
  me: boolean;
  now: number;
  enter: boolean;
}) {
  // Only this row redraws when this person's presence changes.
  const presence = usePresence(instanceKey, m.user?.id);
  const tracked = useOnline(instanceKey) !== null;
  return (
    <motion.div
      initial={enter ? { opacity: 0, x: 16 } : false}
      animate={{ opacity: 1, x: 0 }}
      exit="exit"
      variants={{
        // Someone who left slides out; a line that only scrolled out of view goes at once.
        exit: (present: Set<string>) =>
          present.has(m.user?.id ?? "") ? { opacity: 1, transition: { duration: 0 } } : { opacity: 0, x: 16 },
      }}
      transition={{ type: "spring", stiffness: 500, damping: 36 }}
      className="row-y group flex items-center gap-1 rounded-lg px-2 transition hover:bg-muted/70"
    >
      <span className={tracked && !presence ? "flex min-w-0 flex-1 items-center opacity-45 transition-opacity duration-300 group-hover:opacity-100" : "flex min-w-0 flex-1 items-center transition-opacity duration-300"}>
      <ProfilePopover instanceKey={instanceKey} user={m.user} member={m} side="left">
        <button type="button" className="flex min-w-0 flex-1 items-center gap-2.5 text-left">
          <span className="relative shrink-0 transition duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:scale-105 group-active:scale-95">
            <UserAvatar user={m.user} className="size-8" />
            {tracked && <PresenceDot instanceKey={instanceKey} userId={m.user?.id} hideOffline className="absolute -right-0.5 -bottom-0.5" />}
          </span>
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-1">
              <RoleName id={m.user?.id ?? ""} name={memberName(m)} color={colorOf(roles, m)} className="text-sm" />
              {owner && <CrownIcon aria-label="Owner" className="size-3 shrink-0 text-amber-400" />}
              {isAgent(m.user) && <AppBadge agent />}
              <TimedOutMark member={m} now={now} />
            </span>
            <MemberSubtitle member={m} me={me} now={now} activity={presence?.activities[0]} />
          </span>
        </button>
      </ProfilePopover>
      </span>
      {m.user && (
        <span className="opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
          <CopyId id={m.user.id} what="user ID" />
        </span>
      )}
    </motion.div>
  );
});

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

/** Under a member's name: what they're doing, else their custom status, else their username. */
function MemberSubtitle({ member, me, now, activity }: { member: Member; me: boolean; now: number; activity?: Activity }) {
  const status = shownStatus(member.user, now);
  return (
    <span className="relative block h-4 overflow-hidden text-xs text-muted-foreground">
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={activity ? `a:${activity.kind}:${activity.name}` : status ? `s:${status}` : "username"}
          initial={{ y: 12, opacity: 0 }}
          animate={{ y: 0, opacity: 1 }}
          exit={{ y: -12, opacity: 0 }}
          transition={{ type: "spring", stiffness: 500, damping: 32 }}
          className="block truncate"
        >
          {activity ? (
            <ActivityLine activity={activity} />
          ) : status ? (
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
