import { GavelIcon, LoaderCircleIcon, SearchIcon, ShieldCheckIcon, UndoIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import type { Ban } from "@/gen/fuwa/v1/server_pb";
import type { User } from "@/gen/fuwa/v1/types_pb";
import { listBans, run, unbanMember } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { T, useI18n } from "@/i18n/react";
import { displayName, formatDay, toDate } from "@/lib/format";
import { toast } from "@/lib/ui";

/** Who is kept out of the server, why, and by whom; unbanning lets them join again. */
export function Bans({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const [bans, setBans] = useState<Ban[] | null>(null);
  const [moderators, setModerators] = useState<Record<string, User>>({});
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [lifting, setLifting] = useState<string | null>(null);
  // Someone banned from elsewhere while this is open shows up here too.
  const memberCount = useFuwa((s) => s.instances[instanceKey]?.members[serverId]?.length ?? 0);

  useEffect(() => {
    run(listBans(instanceKey, serverId)).then(
      (res) => {
        setBans(res.bans);
        setModerators(Object.fromEntries(res.moderators.map((u) => [u.id, u])));
      },
      (e: FuwaError) => setError(e.message),
    );
  }, [instanceKey, serverId, memberCount]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!bans || !q) return bans ?? [];
    return bans.filter((b) => `${displayName(b.user)} ${b.user?.username ?? ""} ${b.reason}`.toLowerCase().includes(q));
  }, [bans, query]);

  async function unban(ban: Ban) {
    const id = ban.user?.id ?? "";
    setLifting(id);
    try {
      await run(unbanMember(instanceKey, serverId, id));
      setBans((list) => list?.filter((b) => b.user?.id !== id) ?? null);
      toast(t("serversettings.bans.unbanned", { name: displayName(ban.user) }));
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setLifting(null);
    }
  }

  if (error) return <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>;
  if (!bans) return <div className="flex flex-col gap-2">{[0, 1, 2].map((n) => <div key={n} className="shimmer h-16 rounded-2xl" />)}</div>;
  if (!bans.length)
    return (
      <motion.div initial={{ opacity: 0, scale: 0.96 }} animate={{ opacity: 1, scale: 1 }} transition={SPRING} className="flex flex-col items-center gap-3 py-12 text-center">
        <motion.span
          initial={{ rotate: -20, scale: 0.5 }}
          animate={{ rotate: 0, scale: 1 }}
          transition={{ type: "spring", stiffness: 400, damping: 12, delay: 0.1 }}
          className="grid size-16 place-items-center rounded-3xl bg-primary/10 text-primary"
        >
          <ShieldCheckIcon className="size-8" />
        </motion.span>
        <p className="font-extrabold">{t("serversettings.bans.none")}</p>
        <p className="max-w-xs text-sm text-muted-foreground">{t("serversettings.bans.noneHint")}</p>
      </motion.div>
    );

  return (
    <div className="flex flex-col gap-4">
      <div className="relative">
        <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("serversettings.bans.search")} aria-label={t("serversettings.bans.search")} className="h-10 rounded-xl pl-9" />
      </div>
      <p className="flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
        <GavelIcon className="size-3.5" /> <T k="serversettings.bans.count" values={{ count: <Count value={bans.length} /> }} count={bans.length} />
      </p>
      <ul className="flex flex-col gap-1.5">
        <AnimatePresence initial={false} mode="popLayout">
          {shown.map((ban, n) => {
            const id = ban.user?.id ?? "";
            const by = moderators[ban.bannedById];
            return (
              <motion.li
                key={id}
                layout
                initial={{ opacity: 0, y: 10 }}
                animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: Math.min(n, 12) * 0.03 } }}
                exit={{ opacity: 0, scale: 0.9, x: 40, transition: { duration: 0.25 } }}
                transition={SPRING}
                className="group flex items-start gap-3 rounded-2xl border bg-background/40 p-3"
              >
                <span className="relative shrink-0">
                  <UserAvatar user={ban.user} className="size-10 grayscale transition duration-300 group-hover:grayscale-0" />
                  <span className="absolute -right-1 -bottom-1 grid size-5 place-items-center rounded-full bg-destructive text-white ring-2 ring-background">
                    <GavelIcon className="size-3" />
                  </span>
                </span>
                <div className="min-w-0 flex-1">
                  <p className="truncate font-bold">
                    {displayName(ban.user)} <span className="text-xs font-normal text-muted-foreground">@{ban.user?.username}</span>
                  </p>
                  <p className="text-sm break-words">{ban.reason || <span className="text-muted-foreground italic">{t("serversettings.bans.noReason")}</span>}</p>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    {by
                      ? t("serversettings.bans.bannedBy", { name: displayName(by), date: formatDay(toDate(ban.createdAt)) })
                      : t("serversettings.bans.bannedBySomeone", { date: formatDay(toDate(ban.createdAt)) })}
                  </p>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={lifting === id}
                  onClick={() => void unban(ban)}
                  className="group/unban shrink-0 rounded-xl"
                >
                  {lifting === id ? <LoaderCircleIcon className="animate-spin" /> : <UndoIcon className="transition-transform duration-300 group-hover/unban:-rotate-45" />}
                  {t("serversettings.bans.unban")}
                </Button>
              </motion.li>
            );
          })}
        </AnimatePresence>
      </ul>
      {!shown.length && <p className="py-6 text-center text-sm text-muted-foreground">{t("serversettings.shared.nobodyMatches")}</p>}
    </div>
  );
}
