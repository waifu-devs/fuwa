import { ArrowRightIcon, CrownIcon, LoaderCircleIcon, SearchIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useMemo, useState, type FormEvent } from "react";
import type { Member, Server } from "@/gen/fuwa/v1/types_pb";
import { transferOwnership } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { T, useI18n } from "@/i18n/react";
import { memberName } from "@/lib/format";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const EMPTY: Member[] = [];

/** Hands the server to another member. The owner stays on as an admin. */
export function Ownership({ instanceKey, server, onDone }: { instanceKey: string; server: Server; onDone: () => void }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const members = inst?.members[server.id] ?? EMPTY;
  const me = members.find((m) => m.user?.id === inst?.me?.id);
  const [query, setQuery] = useState("");
  const [picked, setPicked] = useState<Member | null>(null);
  const [confirm, setConfirm] = useState("");
  const transfer = useAction(transferOwnership);
  const others = useMemo(() => {
    const q = query.trim().toLowerCase();
    return members
      .filter((m) => m.user?.id !== me?.user?.id)
      .filter((m) => !q || `${memberName(m)} ${m.user?.username ?? ""}`.toLowerCase().includes(q))
      .slice(0, 8);
  }, [members, me, query]);
  const armed = !!picked && confirm.trim().toLowerCase() === picked.user?.username.toLowerCase();

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!picked?.user || !armed) return;
    if ((await transfer.go(instanceKey, server.id, picked.user.id)) === undefined) return;
    toast(t("serversettings.ownership.done", { name: memberName(picked), server: server.name }));
    onDone();
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-5">
      <p className="text-sm text-muted-foreground">{t("serversettings.ownership.intro")}</p>

      <div className="flex items-center justify-center gap-4 rounded-3xl border bg-background/40 px-4 py-6">
        <Seat member={me} label={t("serversettings.shared.you")} crowned={!picked} />
        <motion.span animate={picked ? { x: [0, 6, 0] } : { x: 0 }} transition={{ duration: 1.2, repeat: picked ? Infinity : 0 }}>
          <ArrowRightIcon className={cn("size-5 transition-colors", picked ? "text-amber-500" : "text-muted-foreground/40")} />
        </motion.span>
        <Seat member={picked ?? undefined} label={picked ? memberName(picked) : t("serversettings.ownership.pick")} crowned={!!picked} />
      </div>

      <div className="flex flex-col gap-2">
        <div className="relative">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("serversettings.ownership.find")} aria-label={t("serversettings.ownership.find")} className="h-10 rounded-xl pl-9" />
        </div>
        <ul role="radiogroup" aria-label={t("serversettings.ownership.newOwner")} className="flex flex-col gap-1">
          {others.map((m) => {
            const active = picked?.user?.id === m.user?.id;
            return (
              <li key={m.user?.id}>
                <button
                  type="button"
                  role="radio"
                  aria-checked={active}
                  onClick={() => {
                    setPicked(m);
                    setConfirm("");
                    transfer.setError(null);
                  }}
                  className={cn(
                    "relative flex w-full items-center gap-3 rounded-xl px-3 py-2 text-left transition-colors",
                    active ? "text-foreground" : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
                  )}
                >
                  {active && <motion.span layoutId="new-owner" transition={SPRING} className="absolute inset-0 rounded-xl bg-amber-500/10 ring-2 ring-amber-500/40" />}
                  <UserAvatar user={m.user} className="relative size-8" />
                  <span className="relative min-w-0 flex-1">
                    <span className="block truncate text-sm font-bold">{memberName(m)}</span>
                    <span className="block truncate text-xs">@{m.user?.username}</span>
                  </span>
                </button>
              </li>
            );
          })}
          {!others.length && <li className="px-3 py-4 text-center text-sm text-muted-foreground">{t("serversettings.ownership.nobody")}</li>}
        </ul>
      </div>

      <AnimatePresence initial={false}>
        {picked && (
          <motion.div
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={SPRING}
            className="overflow-hidden"
          >
            <div className="flex flex-col gap-3 rounded-2xl border border-amber-500/40 bg-amber-500/5 p-4">
              <Label htmlFor="confirm-transfer" className="text-sm">
                <T
                  k="serversettings.ownership.confirm"
                  values={{ username: <b>{picked.user?.username}</b>, server: server.name, name: memberName(picked) }}
                />
              </Label>
              <Input
                id="confirm-transfer"
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
                autoComplete="off"
                className={cn("h-10 rounded-xl transition-colors", armed && "border-amber-500 ring-2 ring-amber-500/20")}
              />
              {transfer.error && <p className="text-sm text-destructive first-letter:uppercase">{transfer.error}</p>}
              <motion.div className="self-end" initial={false} animate={armed ? { scale: [1, 1.08, 1] } : { scale: 1 }} transition={{ duration: 0.35 }}>
                <Button type="submit" disabled={!armed || transfer.pending} className="rounded-xl bg-amber-500 font-bold text-white hover:bg-amber-500/90">
                  {transfer.pending ? <LoaderCircleIcon className="animate-spin" /> : <CrownIcon />} {t("serversettings.nav.ownership")}
                </Button>
              </motion.div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </form>
  );
}

/** Someone on one side of the handover, wearing the crown or not. */
function Seat({ member, label, crowned }: { member: Member | undefined; label: string; crowned: boolean }) {
  return (
    <div className="flex w-28 flex-col items-center gap-2 text-center">
      <div className="relative">
        <AnimatePresence>
          {crowned && (
            <motion.span
              key={member?.user?.id ?? "none"}
              initial={{ y: -24, opacity: 0, rotate: -30 }}
              animate={{ y: 0, opacity: 1, rotate: -12 }}
              exit={{ y: -24, opacity: 0, rotate: 30 }}
              transition={{ type: "spring", stiffness: 420, damping: 14 }}
              className="absolute -top-4 -left-2 z-10 text-amber-400 drop-shadow"
            >
              <CrownIcon className="size-6 fill-amber-400/40" />
            </motion.span>
          )}
        </AnimatePresence>
        {member ? (
          <motion.span key={member.user?.id} initial={{ scale: 0.7 }} animate={{ scale: 1 }} transition={{ type: "spring", stiffness: 500, damping: 18 }} className="block">
            <UserAvatar user={member.user} className="size-16" />
          </motion.span>
        ) : (
          <span className="grid size-16 place-items-center rounded-full border-2 border-dashed text-muted-foreground">?</span>
        )}
      </div>
      <span className="w-full truncate text-sm font-bold">{label}</span>
    </div>
  );
}
