import {
  CheckIcon,
  CrownIcon,
  DoorOpenIcon,
  EllipsisIcon,
  FilterIcon,
  FingerprintIcon,
  GavelIcon,
  HourglassIcon,
  PencilIcon,
  SearchIcon,
  UsersIcon,
} from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Member, Role } from "@/gen/fuwa/v1/types_pb";
import { byId, useAccess, useRoles } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { RoleDot } from "@/components/chat/mentions";
import { UserAvatar } from "@/components/Icons";
import { MemberRoles } from "@/components/MemberRoles";
import { ModerateDialog, type ModAction } from "@/components/ModerateDialog";
import { useNowAtExpiry } from "@/components/use-expiry";
import { useVirtualList } from "@/components/virtual";
import { Count } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { RoleName } from "@/components/RoleName";
import { Segmented } from "@/components/settings/account/common";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { formatDay, formatLeft, formatStamp, memberName, timedOutUntil, toDate } from "@/lib/format";
import { T, useI18n } from "@/i18n/react";
import { useNow } from "@/lib/notifications";
import { type Access, colorOf, moderationFor, standing } from "@/lib/permissions";
import { usePrefs } from "@/lib/prefs";
import { copy } from "@/lib/ui";
import { cn } from "@/lib/utils";

const EMPTY: Member[] = [];

type Filter = "all" | "timed-out";

/**
 * Everyone in the server, highest ranked first, with their roles and what
 * your permissions let you do to each: roles, nicknames, time-outs, kicks
 * and bans, only on people ranked below you.
 *
 * Servers can have thousands of members, so only the rows in view are
 * drawn (`useVirtualList`), and the list moves on only when a time-out
 * ends, not on a ticking clock: the countdowns tick by themselves.
 */
export function Members({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? EMPTY);
  const roles = useRoles(instanceKey, serverId);
  const ownerId = useFuwa((s) => byId(s.instances[instanceKey]?.servers, serverId)?.ownerId ?? "");
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id);
  const access = useAccess(instanceKey, serverId);
  const ends = useMemo(() => members.flatMap((m) => (m.timedOutUntil ? [toDate(m.timedOutUntil).getTime()] : [])), [members]);
  const now = useNowAtExpiry(ends);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [role, setRole] = useState<string | null>(null);
  const [moderating, setModerating] = useState<{ member: Member; action: ModAction } | null>(null);
  const picked = roles.find((r) => r.id === role);
  const onModerate = useCallback((member: Member, action: ModAction) => setModerating({ member, action }), []);

  const timedOut = useMemo(() => members.filter((m) => timedOutUntil(m, now)).length, [members, now]);
  // Highest ranked first; the list is already by name within a rank.
  const ranked = useMemo(
    () => [...members].sort((a, b) => standing(ownerId, roles, b).rank - standing(ownerId, roles, a).rank),
    [members, ownerId, roles],
  );
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return ranked.filter(
      (m) =>
        (filter === "timed-out" ? !!timedOutUntil(m, now) : true) &&
        (!picked || m.roleIds.includes(picked.id)) &&
        (!q || `${memberName(m)} ${m.user?.displayName ?? ""} ${m.user?.username ?? ""}`.toLowerCase().includes(q)),
    );
    // `now` only matters to the timed-out filter.
  }, [ranked, filter, picked, query, filter === "timed-out" ? now : 0]);

  const keys = useMemo(() => shown.map((m) => m.user?.id ?? ""), [shown]);
  const { listRef, measure, total, lines } = useVirtualList(keys, { estimate: 66, gap: 6 });
  // Rows that come into the list (a filter, someone joining) rise in; rows that only scroll into view just appear.
  const known = useRef<Set<string> | null>(null);
  const present = useMemo(() => {
    const ids = new Set(keys);
    const fresh = known.current ? new Set(keys.filter((id) => !known.current!.has(id))) : new Set<string>();
    return { ids, fresh };
  }, [keys]);
  useEffect(() => {
    known.current = present.ids;
  }, [present]);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-0 flex-1 basis-56">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("serversettings.members.search")}
            aria-label={t("serversettings.members.search")}
            className="h-10 rounded-xl pl-9"
          />
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className={cn(
                "flex h-10 items-center gap-2 rounded-xl border px-3 text-sm font-bold transition hover:border-primary/40 data-[state=open]:border-primary/60",
                picked && "border-primary/50 bg-primary/10",
              )}
            >
              {picked ? <RoleDot role={picked} /> : <FilterIcon className="size-4 text-muted-foreground" />}
              <span className="max-w-32 truncate">{picked?.name ?? t("serversettings.members.anyRole")}</span>
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="max-h-72 w-52 overflow-y-auto">
            {[null, ...roles.filter((r) => r.id !== serverId)].map((r) => (
              <DropdownMenuItem key={r?.id ?? "any"} onSelect={() => setRole(r?.id ?? null)}>
                {r ? <RoleDot role={r} /> : <FilterIcon />}
                <span className="flex-1 truncate">{r?.name ?? t("serversettings.members.anyRole")}</span>
                {(r?.id ?? null) === role && <CheckIcon className="size-4 text-primary" />}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        <Segmented
          label={t("serversettings.members.show")}
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: t("serversettings.members.all") },
            { value: "timed-out", label: timedOut ? t("serversettings.members.timedOutCount", { count: timedOut }) : t("serversettings.members.timedOut") },
          ]}
        />
      </div>
      <p className="flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
        <UsersIcon className="size-3.5" /> <T k="serversettings.shared.members" values={{ count: <Count value={shown.length} /> }} count={shown.length} />
      </p>
      <ul ref={listRef as React.RefObject<HTMLUListElement | null>} className="relative" style={{ height: total }}>
        <AnimatePresence initial={false} custom={present.ids} presenceAffectsLayout={false}>
          {lines.map((line) => {
            const m = shown[line.index]!;
            return (
              <li key={line.key} ref={measure} data-vkey={line.key} className="member-line absolute inset-x-0 top-0" style={{ transform: `translateY(${line.top}px)` }}>
                <MemberRow
                  instanceKey={instanceKey}
                  member={m}
                  owner={line.key === ownerId}
                  color={colorOf(roles, m)}
                  timedOut={!!timedOutUntil(m, now)}
                  access={access}
                  ownerId={ownerId}
                  roles={roles}
                  meId={meId}
                  enter={present.fresh.has(line.key)}
                  delay={Math.min(Math.max(0, line.index - (lines[0]?.index ?? 0)), 14) * 0.02}
                  onModerate={onModerate}
                />
              </li>
            );
          })}
        </AnimatePresence>
      </ul>
      <AnimatePresence>
        {shown.length === 0 && (
          <motion.p initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} className="py-8 text-center text-sm text-muted-foreground">
            {filter === "timed-out" && !query && !picked ? t("serversettings.members.noneTimedOut") : t("serversettings.shared.nobodyMatches")}
          </motion.p>
        )}
      </AnimatePresence>
      <ModerateDialog
        instanceKey={instanceKey}
        serverId={serverId}
        member={moderating ? (members.find((m) => m.user?.id === moderating.member.user?.id) ?? moderating.member) : null}
        action={moderating?.action ?? null}
        onClose={() => setModerating(null)}
      />
    </div>
  );
}

/** One member's row: drawn again only when something it shows changes. */
const MemberRow = memo(function MemberRow({
  instanceKey,
  member: m,
  owner,
  color,
  timedOut,
  access,
  ownerId,
  roles,
  meId,
  enter,
  delay,
  onModerate,
}: {
  instanceKey: string;
  member: Member;
  owner: boolean;
  color: number | undefined;
  timedOut: boolean;
  access: Access;
  ownerId: string;
  roles: Role[];
  meId: string | undefined;
  enter: boolean;
  delay: number;
  onModerate: (member: Member, action: ModAction) => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const id = m.user?.id ?? "";
  // "joined today", in lowercase mid-sentence, like the rest of the line.
  const day = formatDay(toDate(m.joinedAt));
  const joined = day === t("common.time.today") || day === t("common.time.yesterday") ? day.toLocaleLowerCase(lang.locale) : day;
  const until = timedOut ? timedOutUntil(m) : null;
  const can = useMemo(() => moderationFor(access, ownerId, roles, meId, m), [access, ownerId, roles, meId, m]);
  const developer = usePrefs((p) => p.developerMode);
  const moderate = useCallback((action: ModAction) => onModerate(m, action), [onModerate, m]);
  return (
    <motion.div
      initial={enter ? { opacity: 0, y: 10 } : false}
      animate={{ opacity: 1, y: 0, x: 0, transition: { ...SPRING, delay: enter ? delay : 0 } }}
      exit="exit"
      variants={{
        // Someone who left the list slides out; a row that only scrolled out of view goes at once.
        exit: (present: Set<string>) => (present.has(id) ? { opacity: 1, transition: { duration: 0 } } : { opacity: 0, x: -24, transition: { duration: 0.2 } }),
      }}
      transition={SPRING}
      className="group flex items-center gap-3 rounded-2xl border bg-background/40 p-2.5 pr-2 transition-colors hover:border-primary/30 hover:bg-muted/40"
    >
      <UserAvatar user={m.user} className="size-10 shrink-0 self-start transition-transform duration-300 group-hover:scale-105" />
      <div className="min-w-0 flex-1">
        <p className="flex min-w-0 items-center gap-1.5">
          <RoleName id={id} name={memberName(m)} color={color} />
          {owner && <CrownIcon aria-label={t("serversettings.shared.owner")} className="size-3.5 shrink-0 text-amber-400" />}
        </p>
        <p className="truncate text-xs text-muted-foreground">
          {t("serversettings.members.line", { username: m.user?.username ?? "", date: joined })}
        </p>
        <div className="mt-1.5 empty:hidden">
          <MemberRoles instanceKey={instanceKey} member={m} compact />
        </div>
      </div>
      <AnimatePresence>
        {until && (
          <motion.button
            type="button"
            initial={{ scale: 0.6, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            exit={{ scale: 0.6, opacity: 0 }}
            transition={SPRING}
            disabled={!can.timeout}
            onClick={() => moderate("timeout")}
            title={t("serversettings.members.timedOutUntil", { time: formatStamp(until) })}
            className="flex shrink-0 items-center gap-1 rounded-full bg-amber-500/15 px-2 py-1 text-xs font-bold text-amber-600 tabular-nums transition enabled:hover:bg-amber-500/25 dark:text-amber-400"
          >
            <HourglassIcon className="size-3.5 animate-[spin_3s_ease-in-out_infinite]" />
            <TimeLeft until={until} />
          </motion.button>
        )}
      </AnimatePresence>
      {(can.any || developer) && (
        <MemberMenu id={id} name={memberName(m)} can={can} timedOut={!!until} developer={developer} onModerate={moderate} />
      )}
    </motion.div>
  );
});

/** A time-out's countdown: the one part of a row that ticks every second. */
function TimeLeft({ until }: { until: Date }) {
  const lang = useI18n();
  const now = useNow(1000);
  return <>{formatLeft(lang, Math.max(0, until.getTime() - now))}</>;
}

/** A member's "…" menu: what you may do to them, and copying their id in developer mode. */
function MemberMenu({
  id,
  name,
  can,
  timedOut,
  developer,
  onModerate,
}: {
  id: string;
  name: string;
  can: ReturnType<typeof moderationFor>;
  timedOut: boolean;
  developer: boolean;
  onModerate: (action: ModAction) => void;
}) {
  const { t } = useI18n();
  const punish = can.timeout || can.kick || can.ban;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={t("serversettings.members.actionsFor", { name })}
          className="grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition hover:bg-muted hover:text-foreground data-[state=open]:bg-muted data-[state=open]:text-foreground"
        >
          <EllipsisIcon className="size-4 transition-transform duration-300 group-hover:rotate-90" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        {can.nickname && (
          <DropdownMenuItem onSelect={() => onModerate("nickname")}>
            <PencilIcon /> {t("serversettings.members.changeNickname")}
          </DropdownMenuItem>
        )}
        {can.nickname && punish && <DropdownMenuSeparator />}
        {can.timeout && (
          <DropdownMenuItem onSelect={() => onModerate("timeout")}>
            <HourglassIcon /> {timedOut ? t("serversettings.members.changeTimeout") : t("serversettings.members.timeOut")}
          </DropdownMenuItem>
        )}
        {can.kick && (
          <DropdownMenuItem variant="destructive" onSelect={() => onModerate("kick")}>
            <DoorOpenIcon /> {t("serversettings.members.kick")}
          </DropdownMenuItem>
        )}
        {can.ban && (
          <DropdownMenuItem variant="destructive" onSelect={() => onModerate("ban")}>
            <GavelIcon /> {t("serversettings.members.ban")}
          </DropdownMenuItem>
        )}
        {developer && can.any && <DropdownMenuSeparator />}
        {developer && (
          <DropdownMenuItem onSelect={() => copy(t, id, t("common.copy.userId"))}>
            <FingerprintIcon /> {t("common.copyThing", { what: t("common.copy.userId") })}
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
