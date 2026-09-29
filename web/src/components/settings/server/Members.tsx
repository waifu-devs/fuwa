import {
  CrownIcon,
  DoorOpenIcon,
  EllipsisIcon,
  FingerprintIcon,
  GavelIcon,
  HourglassIcon,
  PencilIcon,
  SearchIcon,
  ShieldIcon,
  ShieldOffIcon,
  UsersIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useMemo, useState, type CSSProperties } from "react";
import { MemberRole, type Member } from "@/gen/fuwa/v1/types_pb";
import { run, setRole } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { ModerateDialog, outranks, type ModAction } from "@/components/ModerateDialog";
import { Count, SPRING } from "@/components/motion";
import { Segmented } from "@/components/settings/account/common";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { formatDay, formatLeft, formatStamp, hueOf, memberName, timedOutUntil, toDate } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { usePrefs } from "@/lib/prefs";
import { copy, toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const EMPTY: Member[] = [];

type Filter = "all" | "admins" | "timed-out";

/** Everyone in the server, with what owners and admins can do to each. */
export function Members({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const inst = useInstance(instanceKey);
  const members = inst?.members[serverId] ?? EMPTY;
  const me = members.find((m) => m.user?.id === inst?.me?.id);
  const owner = me?.role === MemberRole.OWNER;
  const now = useNow(1000);
  const developer = usePrefs((p) => p.developerMode);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [moderating, setModerating] = useState<{ member: Member; action: ModAction } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const timedOut = members.filter((m) => timedOutUntil(m, now)).length;
  const shown = useMemo(() => {
    const kept = members.filter((m) =>
      filter === "admins" ? m.role >= MemberRole.ADMIN : filter === "timed-out" ? !!timedOutUntil(m, now) : true,
    );
    const q = query.trim().toLowerCase();
    if (!q) return kept;
    return kept.filter((m) => `${memberName(m)} ${m.user?.displayName ?? ""} ${m.user?.username ?? ""}`.toLowerCase().includes(q));
  }, [members, filter, query, now]);

  async function changeRole(member: Member, role: MemberRole) {
    const id = member.user?.id ?? "";
    setBusy(id);
    try {
      await run(setRole(instanceKey, serverId, id, role));
      toast(role === MemberRole.ADMIN ? `${memberName(member)} is an admin now` : `${memberName(member)} is a member now`);
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-0 flex-1 basis-56">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search members"
            aria-label="Search members"
            className="h-10 rounded-xl pl-9"
          />
        </div>
        <Segmented
          label="Show"
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: "All" },
            { value: "admins", label: "Admins" },
            { value: "timed-out", label: timedOut ? `Timed out · ${timedOut}` : "Timed out" },
          ]}
        />
      </div>
      <p className="flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
        <UsersIcon className="size-3.5" /> <Count value={shown.length} /> {shown.length === 1 ? "member" : "members"}
      </p>
      <ul className="flex flex-col gap-1.5">
        <AnimatePresence initial={false} mode="popLayout">
          {shown.map((m, n) => {
            const id = m.user?.id ?? "";
            const until = timedOutUntil(m, now);
            const can = outranks(me, m);
            const RoleIcon = m.role === MemberRole.OWNER ? CrownIcon : m.role === MemberRole.ADMIN ? ShieldIcon : null;
            return (
              <motion.li
                key={id}
                layout
                initial={{ opacity: 0, y: 10 }}
                animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: Math.min(n, 14) * 0.02 } }}
                exit={{ opacity: 0, x: -24, transition: { duration: 0.2 } }}
                transition={SPRING}
                className="group flex items-center gap-3 rounded-2xl border bg-background/40 p-2.5 pr-2 transition-colors hover:border-primary/30 hover:bg-muted/40"
              >
                <UserAvatar user={m.user} className="size-10 shrink-0 transition-transform duration-300 group-hover:scale-105" />
                <div className="min-w-0 flex-1">
                  <p className="flex min-w-0 items-center gap-1.5">
                    <span className="name-tint truncate font-bold" style={{ "--h": hueOf(id) } as CSSProperties}>
                      {memberName(m)}
                    </span>
                    <AnimatePresence mode="popLayout" initial={false}>
                      {RoleIcon && (
                        <motion.span
                          key={m.role}
                          initial={{ scale: 0, rotate: -40 }}
                          animate={{ scale: 1, rotate: 0 }}
                          exit={{ scale: 0, rotate: 40 }}
                          transition={{ type: "spring", stiffness: 600, damping: 16 }}
                          className={cn("shrink-0", m.role === MemberRole.OWNER ? "text-amber-400" : "text-primary")}
                          title={m.role === MemberRole.OWNER ? "Owner" : "Admin"}
                        >
                          <RoleIcon className="size-3.5" />
                        </motion.span>
                      )}
                    </AnimatePresence>
                  </p>
                  <p className="truncate text-xs text-muted-foreground">
                    @{m.user?.username} · joined {formatDay(toDate(m.joinedAt)).replace(/^(Today|Yesterday)$/, (d) => d.toLowerCase())}
                  </p>
                </div>
                <AnimatePresence>
                  {until && (
                    <motion.button
                      type="button"
                      initial={{ scale: 0.6, opacity: 0 }}
                      animate={{ scale: 1, opacity: 1 }}
                      exit={{ scale: 0.6, opacity: 0 }}
                      transition={SPRING}
                      disabled={!can}
                      onClick={() => setModerating({ member: m, action: "timeout" })}
                      title={`Timed out until ${formatStamp(until)}`}
                      className="flex shrink-0 items-center gap-1 rounded-full bg-amber-500/15 px-2 py-1 text-xs font-bold text-amber-600 tabular-nums transition enabled:hover:bg-amber-500/25 dark:text-amber-400"
                    >
                      <HourglassIcon className="size-3.5 animate-[spin_3s_ease-in-out_infinite]" />
                      {formatLeft(until.getTime() - now)}
                    </motion.button>
                  )}
                </AnimatePresence>
                {(can || developer) && (
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <button
                        type="button"
                        aria-label={`Actions for ${memberName(m)}`}
                        disabled={busy === id}
                        className="grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition hover:bg-muted hover:text-foreground data-[state=open]:bg-muted data-[state=open]:text-foreground"
                      >
                        <EllipsisIcon className="size-4 transition-transform duration-300 group-hover:rotate-90" />
                      </button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end" className="w-52">
                      {can && (
                        <DropdownMenuItem onSelect={() => setModerating({ member: m, action: "nickname" })}>
                          <PencilIcon /> Change nickname
                        </DropdownMenuItem>
                      )}
                      {can && owner && m.role === MemberRole.MEMBER && (
                        <DropdownMenuItem onSelect={() => void changeRole(m, MemberRole.ADMIN)}>
                          <ShieldIcon /> Make admin
                        </DropdownMenuItem>
                      )}
                      {can && owner && m.role === MemberRole.ADMIN && (
                        <DropdownMenuItem onSelect={() => void changeRole(m, MemberRole.MEMBER)}>
                          <ShieldOffIcon /> Remove admin
                        </DropdownMenuItem>
                      )}
                      {can && <DropdownMenuSeparator />}
                      {can && (
                        <DropdownMenuItem onSelect={() => setModerating({ member: m, action: "timeout" })}>
                          <HourglassIcon /> {until ? "Change time-out" : "Time out"}
                        </DropdownMenuItem>
                      )}
                      {can && (
                        <DropdownMenuItem variant="destructive" onSelect={() => setModerating({ member: m, action: "kick" })}>
                          <DoorOpenIcon /> Kick
                        </DropdownMenuItem>
                      )}
                      {can && (
                        <DropdownMenuItem variant="destructive" onSelect={() => setModerating({ member: m, action: "ban" })}>
                          <GavelIcon /> Ban
                        </DropdownMenuItem>
                      )}
                      {developer && can && <DropdownMenuSeparator />}
                      {developer && (
                        <DropdownMenuItem onSelect={() => copy(id, "user ID")}>
                          <FingerprintIcon /> Copy user ID
                        </DropdownMenuItem>
                      )}
                    </DropdownMenuContent>
                  </DropdownMenu>
                )}
              </motion.li>
            );
          })}
        </AnimatePresence>
      </ul>
      <AnimatePresence>
        {shown.length === 0 && (
          <motion.p initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} className="py-8 text-center text-sm text-muted-foreground">
            {filter === "timed-out" && !query ? "Nobody is timed out." : "Nobody matches that."}
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
