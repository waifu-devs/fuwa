import * as DialogPrimitive from "@radix-ui/react-dialog";
import { CheckIcon, ClipboardPenIcon, EyeIcon, HourglassIcon, PartyPopperIcon, SendIcon, Undo2Icon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import type { ReactNode } from "react";
import { ApplicationStatus, type Server } from "@/gen/fuwa/v1/types_pb";
import { withdrawApplication } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { BannerHero } from "@/components/join/Banner";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { type I18n, useI18n } from "@/i18n/react";
import { accentVars } from "@/lib/banner";
import { ago } from "@/lib/format";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Where an application stands, as the card shows it. */
export type Standing = "waiting" | "accepted" | "declined";

/** Where your application to a server stands, from what this browser knows: let in, turned down or waiting. */
export function useStanding(instanceKey: string, serverId: string): { standing: Standing; appliedAt: number | null; reason: string } {
  const inst = useInstance(instanceKey);
  const applied = inst?.applied[serverId];
  if (inst?.servers.some((s) => s.id === serverId)) return { standing: "accepted", appliedAt: applied?.appliedAt ?? null, reason: "" };
  if (applied?.status === ApplicationStatus.REJECTED) return { standing: "declined", appliedAt: applied.appliedAt, reason: applied.reason };
  return { standing: "waiting", appliedAt: applied?.appliedAt ?? null, reason: "" };
}

type Stop = { id: string; icon: ReactNode; title: string; note: string; state: "done" | "now" | "later" | "no" };

/** The three stops, as far as the application has got. */
function timelineStops(lang: I18n, standing: Standing, appliedAt: number | null, reason: string): Stop[] {
  const { t } = lang;
  const done = standing !== "waiting";
  return [
    { id: "sent", icon: <SendIcon className="size-3.5" />, title: t("join.timeline.sent"), note: appliedAt ? t("join.timeline.appliedAgo", { when: ago(lang, new Date(appliedAt)) }) : t("join.timeline.applied"), state: "done" },
    {
      id: "read",
      icon: done ? <EyeIcon className="size-3.5" /> : <HourglassIcon className="size-3.5 animate-[flip_3s_ease-in-out_infinite]" />,
      title: done ? t("join.timeline.read") : t("join.timeline.beingRead"),
      note: done ? t("join.timeline.readNote") : t("join.timeline.beingReadNote"),
      state: done ? "done" : "now",
    },
    standing === "declined"
      ? { id: "answer", icon: <XIcon className="size-3.5" strokeWidth={3} />, title: t("join.timeline.turnedDown"), note: reason ? "" : t("join.timeline.noReason"), state: "no" }
      : {
          id: "answer",
          icon: standing === "accepted" ? <PartyPopperIcon className="size-3.5" /> : <CheckIcon className="size-3.5" />,
          title: standing === "accepted" ? t("join.timeline.youreIn") : t("join.timeline.letIn"),
          note: standing === "accepted" ? t("join.timeline.inYourList") : t("join.timeline.letInNote"),
          state: standing === "accepted" ? "done" : "later",
        },
  ];
}

/**
 * Where an application stands, as three stops on a line: sent, being read,
 * and the answer. The line fills as it moves on, the stop it's at pulses
 * while it waits, and the answer pops in with the reason when there is one.
 */
export function ApplicationTimeline({ standing, appliedAt, reason }: { standing: Standing; appliedAt: number | null; reason: string }) {
  const lang = useI18n();
  const stops = timelineStops(lang, standing, appliedAt, reason);
  const filled = standing === "waiting" ? 0.5 : 1;
  return (
    <ol className="relative flex flex-col gap-4">
      <span aria-hidden className="absolute top-3 bottom-3 left-3 w-0.5 -translate-x-1/2 rounded-full bg-muted" />
      <motion.span
        aria-hidden
        initial={{ scaleY: 0 }}
        animate={{ scaleY: filled }}
        transition={{ ...SPRING, delay: 0.15 }}
        className={cn("absolute top-3 bottom-3 left-3 w-0.5 origin-top -translate-x-1/2 rounded-full", standing === "declined" ? "bg-destructive/60" : "bg-[var(--accent-server)]")}
      />
      {stops.map((s, n) => (
        <TimelineStop key={`${s.id}:${s.state}`} stop={s} n={n} reason={reason} />
      ))}
    </ol>
  );
}

/** One stop on the timeline: its dot, what it is, and the reason under a turn-down. */
function TimelineStop({ stop, n, reason }: { stop: Stop; n: number; reason: string }) {
  const { t } = useI18n();
  return (
    <motion.li
      initial={{ opacity: 0, x: -10 }}
      animate={{ opacity: 1, x: 0 }}
      transition={{ ...SPRING, delay: 0.1 + n * 0.08 }}
      className="relative flex items-start gap-3"
    >
      <span
        className={cn(
          "relative z-10 grid size-6 shrink-0 place-items-center rounded-full ring-4 ring-card",
          stop.state === "done" && "bg-[var(--accent-server)] text-white",
          stop.state === "now" && "bg-amber-500 text-white",
          stop.state === "later" && "bg-muted text-muted-foreground",
          stop.state === "no" && "bg-destructive text-white",
        )}
      >
        {stop.state === "now" && <span className="absolute inset-0 animate-ping rounded-full bg-amber-500/50 motion-reduce:hidden" />}
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span key={stop.state} initial={{ scale: 0, rotate: -60 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 600, damping: 16 }} className="relative grid">
            {stop.icon}
          </motion.span>
        </AnimatePresence>
      </span>
      <div className="min-w-0 pt-0.5">
        <p className={cn("text-sm font-extrabold", stop.state === "later" && "text-muted-foreground")}>{stop.title}</p>
        {stop.note && <p className="text-xs text-muted-foreground">{stop.note}</p>}
        {stop.state === "no" && reason && (
          <motion.blockquote
            initial={{ opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ ...SPRING, delay: 0.35 }}
            className="mt-1.5 rounded-xl border-l-4 border-destructive/60 bg-destructive/5 px-3 py-2 text-sm break-words"
          >
            {t("join.quoted", { text: reason })}
          </motion.blockquote>
        )}
      </div>
    </motion.li>
  );
}

/**
 * Your application to a server under its banner: where it stands, and what
 * you can do about it (take it back, apply again, open the server).
 */
export function ApplicationCard({
  instanceKey,
  server,
  onApplyAgain,
  onOpen,
  onDone,
}: {
  instanceKey: string;
  server: Server;
  onApplyAgain?: () => void;
  onOpen?: () => void;
  onDone: () => void;
}) {
  const { standing, appliedAt, reason } = useStanding(instanceKey, server.id);
  const withdraw = useAction(withdrawApplication);
  const { t } = useI18n();
  return (
    <div style={accentVars(server)} className="flex flex-col gap-5">
      <BannerHero
        server={server}
        bleed
        eyebrow={standing === "accepted" ? t("join.card.eyebrowIn") : standing === "declined" ? t("join.card.eyebrowDeclined") : t("join.card.eyebrowWaiting")}
        badge={standing === "waiting" ? <HourglassIcon className="size-3.5 animate-[flip_3s_ease-in-out_infinite]" /> : standing === "accepted" ? <CheckIcon className="size-3.5" strokeWidth={3} /> : <XIcon className="size-3.5" strokeWidth={3} />}
      />
      <DialogPrimitive.Title className="sr-only">{t("join.card.title", { server: server.name })}</DialogPrimitive.Title>
      <DialogPrimitive.Description className="sr-only">{t("join.card.description")}</DialogPrimitive.Description>
      <ApplicationTimeline standing={standing} appliedAt={appliedAt} reason={reason} />
      {withdraw.error && <p className="text-sm text-destructive first-letter:uppercase">{withdraw.error}</p>}
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.div key={standing} initial={{ opacity: 0, scale: 0.95 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.95 }} transition={SPRING} className="flex flex-col gap-2 sm:flex-row-reverse">
          {standing === "accepted" ? (
            <Button onClick={onOpen ?? onDone} style={{ background: "var(--accent-server)" }} className="h-10 flex-1 rounded-xl font-bold text-white hover:brightness-110" data-burst="">
              <PartyPopperIcon /> {t("join.card.open", { server: server.name })}
            </Button>
          ) : standing === "declined" ? (
            <>
              {onApplyAgain && (
                <Button onClick={onApplyAgain} className="btn h-10 flex-1 rounded-xl font-bold">
                  <ClipboardPenIcon /> {t("join.applyAgain")}
                </Button>
              )}
              <Button variant="outline" onClick={onDone} className="h-10 flex-1 rounded-xl font-bold">
                {t("common.close")}
              </Button>
            </>
          ) : (
            <>
              <Button onClick={onDone} className="btn h-10 flex-1 rounded-xl font-bold">
                {t("join.card.gotIt")}
              </Button>
              <Button
                variant="ghost"
                disabled={withdraw.pending}
                onClick={async () => {
                  if ((await withdraw.go(instanceKey, server.id)) === undefined) return;
                  toast(t("join.withdrawn", { server: server.name }));
                  onDone();
                }}
                className="h-10 flex-1 rounded-xl font-bold text-muted-foreground hover:text-destructive"
              >
                <Undo2Icon /> {t("join.card.takeBack")}
              </Button>
            </>
          )}
        </motion.div>
      </AnimatePresence>
    </div>
  );
}

/** Your application to a server, in a dialog of its own: opened from the rail. */
export function ApplicationDialog({
  open,
  onOpenChange,
  instanceKey,
  server,
  onApplyAgain,
  onOpen,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  server: Server;
  onApplyAgain?: () => void;
  onOpen?: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="overflow-x-hidden">
        <ApplicationCard instanceKey={instanceKey} server={server} onApplyAgain={onApplyAgain} onOpen={onOpen} onDone={() => onOpenChange(false)} />
      </DialogContent>
    </Dialog>
  );
}
