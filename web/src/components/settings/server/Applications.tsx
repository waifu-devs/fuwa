import { CheckIcon, InboxIcon, LoaderCircleIcon, SparklesIcon, UserCheckIcon, UserXIcon } from "lucide-react";
import { AnimatePresence, m as motion, type Variants } from "motion/react";
import { useEffect, useState, type Ref } from "react";
import type { Application } from "@/gen/fuwa/v1/types_pb";
import { listApplications, reviewApplication, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { Count, SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ago, displayName, roughly, toDate } from "@/lib/format";
import { T, useI18n } from "@/i18n/react";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Accounts younger than this get a "new account" tag, to spot throwaways. */
const NEW_ACCOUNT_MS = 7 * 86_400_000;
const REASON_MAX = 512;

type Decision = { userId: string; approve: boolean };

/**
 * People asking to join, oldest first, with their answers. Letting someone in
 * sends their card off to the right; turning them down (with a reason they'll
 * see) sends it off to the left. New ones slide in as they arrive.
 */
export function Applications({ instanceKey, serverId, takesApplications }: { instanceKey: string; serverId: string; takesApplications: boolean }) {
  const { t } = useI18n();
  const list = useFuwa((s) => s.instances[instanceKey]?.applications[serverId]);
  const [error, setError] = useState<string | null>(null);
  const [decided, setDecided] = useState<Decision | null>(null);

  useEffect(() => {
    run(listApplications(instanceKey, serverId)).then(
      () => setError(null),
      (e: FuwaError) => setError(e.message),
    );
  }, [instanceKey, serverId]);

  if (error && !list) return <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>;
  if (!list) return <div className="flex flex-col gap-3">{[0, 1].map((n) => <div key={n} className="shimmer h-40 rounded-3xl" />)}</div>;

  return (
    <div className="flex flex-col gap-3">
      {list.length > 0 && (
        <p className="text-sm text-muted-foreground">
          <T
            k="serversettings.applications.waiting"
            values={{
              people: (
                <b className="text-foreground">
                  <T k="serversettings.applications.people" values={{ count: <Count value={list.length} /> }} count={list.length} />
                </b>
              ),
            }}
          />
        </p>
      )}
      <AnimatePresence mode="popLayout" custom={decided}>
        {list.length === 0 ? (
          <motion.div
            key="empty"
            initial={{ opacity: 0, scale: 0.96 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.96 }}
            transition={SPRING}
            className="flex flex-col items-center gap-3 rounded-3xl border border-dashed py-12 text-center"
          >
            <span className="float grid size-14 place-items-center rounded-full bg-primary/15 text-primary">
              <InboxIcon className="size-7" />
            </span>
            <p className="font-extrabold">{t("serversettings.applications.none")}</p>
            <p className="max-w-sm text-sm text-muted-foreground">
              {takesApplications ? t("serversettings.applications.noneHint") : t("serversettings.applications.noneHintOff")}
            </p>
          </motion.div>
        ) : (
          list.map((a) => (
            <Card
              key={a.user?.id}
              instanceKey={instanceKey}
              serverId={serverId}
              application={a}
              onDecided={(approve) => setDecided({ userId: a.user?.id ?? "", approve })}
            />
          ))
        )}
      </AnimatePresence>
    </div>
  );
}

/** Off to the right when let in, to the left when turned down; others just fade. */
const leave: Variants = {
  exit: (d: Decision | null) => ({
    opacity: 0,
    x: d ? (d.approve ? 120 : -120) : 0,
    rotate: d ? (d.approve ? 4 : -4) : 0,
    scale: 0.96,
    transition: { duration: 0.35, ease: [0.4, 0, 1, 1] as const },
  }),
};

function Card({
  instanceKey,
  serverId,
  application: a,
  onDecided,
  ref,
}: {
  instanceKey: string;
  serverId: string;
  application: Application;
  onDecided: (approve: boolean) => void;
  /** For AnimatePresence, so the card leaves the layout as it flies off. */
  ref?: Ref<HTMLElement>;
}) {
  const lang = useI18n();
  const { t } = lang;
  const [declining, setDeclining] = useState(false);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState<"in" | "out" | null>(null);
  const [flash, setFlash] = useState<"in" | "out" | null>(null);
  const name = displayName(a.user);
  const made = toDate(a.accountCreatedAt);
  const fresh = Date.now() - made.getTime() < NEW_ACCOUNT_MS;

  async function decide(approve: boolean) {
    setBusy(approve ? "in" : "out");
    try {
      onDecided(approve);
      setFlash(approve ? "in" : "out");
      await run(reviewApplication(instanceKey, serverId, a, approve, approve ? "" : reason.trim()));
      toast(approve ? t("serversettings.applications.letInDone", { name }) : t("serversettings.applications.turnedDownDone", { name }));
    } catch (err) {
      setFlash(null);
      toast((err as FuwaError).message);
    } finally {
      setBusy(null);
    }
  }

  return (
    <motion.article
      ref={ref}
      layout
      variants={leave}
      initial={{ opacity: 0, y: 16, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit="exit"
      transition={SPRING}
      className={cn(
        "relative overflow-hidden rounded-3xl border bg-card p-4 transition-colors sm:p-5",
        flash === "in" && "border-emerald-500/60 bg-emerald-500/5",
        flash === "out" && "border-destructive/50 bg-destructive/5",
      )}
    >
      <header className="flex items-center gap-3">
        <UserAvatar user={a.user} className="size-11" />
        <div className="min-w-0 flex-1">
          <p className="flex items-center gap-2 truncate font-extrabold">
            {name}
            {fresh && (
              <span className="flex shrink-0 items-center gap-1 rounded-full bg-amber-500/15 px-2 py-0.5 text-[0.65rem] font-bold text-amber-600 dark:text-amber-400">
                <SparklesIcon className="size-3" /> {t("serversettings.applications.newAccount")}
              </span>
            )}
          </p>
          <p className="truncate text-xs text-muted-foreground">
            {t("serversettings.applications.line", {
              username: a.user?.username ?? "",
              age: roughly(lang, Date.now() - made.getTime()),
              when: ago(lang, toDate(a.createdAt)),
            })}
          </p>
        </div>
      </header>

      {a.answers.length > 0 && (
        <dl className="mt-4 flex flex-col gap-3">
          {a.answers.map((answer, n) => (
            <div key={n} className="rounded-2xl bg-muted/50 p-3">
              <dt className="text-xs font-bold text-muted-foreground">{answer.question}</dt>
              <dd className={cn("mt-1 text-sm break-words whitespace-pre-wrap", !answer.answer && "text-muted-foreground italic")}>{answer.answer || t("serversettings.applications.noAnswer")}</dd>
            </div>
          ))}
        </dl>
      )}

      <AnimatePresence initial={false}>
        {declining && (
          <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
            <label htmlFor={`reason-${a.user?.id}`} className="mt-4 flex items-center justify-between text-sm font-bold">
              {t("serversettings.applications.whyNot")} <span className="text-xs font-normal text-muted-foreground">{t("serversettings.applications.whyNotHint")}</span>
            </label>
            <Textarea
              id={`reason-${a.user?.id}`}
              autoFocus
              rows={2}
              maxLength={REASON_MAX}
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              className="mt-1.5 rounded-xl"
            />
          </motion.div>
        )}
      </AnimatePresence>

      <footer className="mt-4 flex flex-wrap items-center justify-end gap-2">
        {declining ? (
          <>
            <Button variant="ghost" disabled={!!busy} onClick={() => setDeclining(false)} className="rounded-xl">
              {t("common.cancel")}
            </Button>
            <Button variant="destructive" disabled={!!busy} onClick={() => void decide(false)} className="rounded-xl font-bold">
              {busy === "out" ? <LoaderCircleIcon className="animate-spin" /> : <UserXIcon />} {t("serversettings.applications.turnDown")}
            </Button>
          </>
        ) : (
          <>
            <Button variant="outline" disabled={!!busy} onClick={() => setDeclining(true)} className="rounded-xl font-bold hover:border-destructive/50 hover:text-destructive">
              <UserXIcon /> {t("serversettings.applications.turnDown")}
            </Button>
            <Button disabled={!!busy} onClick={() => void decide(true)} className="group rounded-xl bg-emerald-500 font-bold text-white hover:bg-emerald-600" data-burst>
              {busy === "in" ? (
                <LoaderCircleIcon className="animate-spin" />
              ) : flash === "in" ? (
                <CheckIcon strokeWidth={3} />
              ) : (
                <UserCheckIcon className="transition-transform group-hover:scale-110" />
              )}
              {t("serversettings.applications.letIn")}
            </Button>
          </>
        )}
      </footer>
    </motion.article>
  );
}
