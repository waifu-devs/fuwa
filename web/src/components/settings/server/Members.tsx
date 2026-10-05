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
import { AnimatePresence, motion } from "motion/react";
import { useMemo, useState } from "react";
import type { Member } from "@/gen/fuwa/v1/types_pb";
import { useInstance, useRoles } from "@/fuwa/hooks";
import { RoleDot } from "@/components/chat/mentions";
import { UserAvatar } from "@/components/Icons";
import { MemberRoles } from "@/components/MemberRoles";
import { ModerateDialog, useModeration, type ModAction } from "@/components/ModerateDialog";
import { Count, SPRING } from "@/components/motion";
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
import { colorOf, standing } from "@/lib/permissions";
import { usePrefs } from "@/lib/prefs";
import { copy } from "@/lib/ui";
import { cn } from "@/lib/utils";

const EMPTY: Member[] = [];

type Filter = "all" | "timed-out";

/**
 * Everyone in the server, highest ranked first, with their roles and what
 * your permissions let you do to each: roles, nicknames, time-outs, kicks
 * and bans, only on people ranked below you.
 */
export function Members({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const members = inst?.members[serverId] ?? EMPTY;
  const roles = useRoles(instanceKey, serverId);
  const ownerId = inst?.servers.find((s) => s.id === serverId)?.ownerId ?? "";
  const now = useNow(1000);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [role, setRole] = useState<string | null>(null);
  const [moderating, setModerating] = useState<{ member: Member; action: ModAction } | null>(null);
  const picked = roles.find((r) => r.id === role);

  const timedOut = members.filter((m) => timedOutUntil(m, now)).length;
  const shown = useMemo(() => {
    const kept = members.filter(
      (m) => (filter === "timed-out" ? !!timedOutUntil(m, now) : true) && (!picked || m.roleIds.includes(picked.id)),
    );
    const q = query.trim().toLowerCase();
    const found = q
      ? kept.filter((m) => `${memberName(m)} ${m.user?.displayName ?? ""} ${m.user?.username ?? ""}`.toLowerCase().includes(q))
      : kept;
    // Highest ranked first; the list is already by name within a rank.
    return [...found].sort((a, b) => standing(ownerId, roles, b).rank - standing(ownerId, roles, a).rank);
  }, [members, filter, picked, query, now, ownerId, roles]);

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
      <ul className="flex flex-col gap-1.5">
        <AnimatePresence initial={false} mode="popLayout">
          {shown.map((m, n) => (
            <MemberRow
              key={m.user?.id}
              index={n}
              instanceKey={instanceKey}
              member={m}
              owner={m.user?.id === ownerId}
              color={colorOf(roles, m)}
              now={now}
              onModerate={(action) => setModerating({ member: m, action })}
            />
          ))}
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

function MemberRow({
  index,
  instanceKey,
  member: m,
  owner,
  color,
  now,
  onModerate,
}: {
  index: number;
  instanceKey: string;
  member: Member;
  owner: boolean;
  color: number | undefined;
  now: number;
  onModerate: (action: ModAction) => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const id = m.user?.id ?? "";
  // "joined today", in lowercase mid-sentence, like the rest of the line.
  const day = formatDay(toDate(m.joinedAt));
  const joined = day === t("common.time.today") || day === t("common.time.yesterday") ? day.toLocaleLowerCase(lang.locale) : day;
  const until = timedOutUntil(m, now);
  const can = useModeration(instanceKey, m.serverId, m);
  const developer = usePrefs((p) => p.developerMode);
  const punish = can.timeout || can.kick || can.ban;
  return (
    <motion.li
      layout
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: Math.min(index, 14) * 0.02 } }}
      exit={{ opacity: 0, x: -24, transition: { duration: 0.2 } }}
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
            onClick={() => onModerate("timeout")}
            title={t("serversettings.members.timedOutUntil", { time: formatStamp(until) })}
            className="flex shrink-0 items-center gap-1 rounded-full bg-amber-500/15 px-2 py-1 text-xs font-bold text-amber-600 tabular-nums transition enabled:hover:bg-amber-500/25 dark:text-amber-400"
          >
            <HourglassIcon className="size-3.5 animate-[spin_3s_ease-in-out_infinite]" />
            {formatLeft(lang, until.getTime() - now)}
          </motion.button>
        )}
      </AnimatePresence>
      {(can.any || developer) && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label={t("serversettings.members.actionsFor", { name: memberName(m) })}
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
                <HourglassIcon /> {until ? t("serversettings.members.changeTimeout") : t("serversettings.members.timeOut")}
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
              <DropdownMenuItem onSelect={() => copy(lang.t, id, lang.t("common.copy.userId"))}>
                <FingerprintIcon /> {t("common.copyThing", { what: t("common.copy.userId") })}
              </DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </motion.li>
  );
}
