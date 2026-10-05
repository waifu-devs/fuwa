import { LoaderCircleIcon, PartyPopperIcon, ScrollTextIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useState, type ReactNode } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { agreeToRules, getJoinForm, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction } from "@/fuwa/hooks";
import { ServerIcon } from "@/components/Icons";
import { InlineMarkdown } from "@/components/Markdown";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { useI18n } from "@/i18n/react";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** A server's rules, numbered, each sliding in just after the one before. */
export function RulesList({ rules, className }: { rules: string[]; className?: string }) {
  return (
    <ol className={cn("flex flex-col gap-2", className)}>
      <AnimatePresence initial={true} mode="popLayout">
        {rules.map((rule, n) => (
          <motion.li
            key={`${n}:${rule}`}
            layout="position"
            initial={{ opacity: 0, x: -14 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, scale: 0.95 }}
            transition={{ ...SPRING, delay: Math.min(n, 10) * 0.045 }}
            className="flex items-start gap-3 rounded-2xl bg-muted/50 p-3 text-sm"
          >
            <span className="grid size-6 shrink-0 place-items-center rounded-full bg-primary/15 text-xs font-extrabold text-primary tabular-nums">{n + 1}</span>
            <InlineMarkdown className="min-w-0 flex-1 pt-0.5 break-words">{rule}</InlineMarkdown>
          </motion.li>
        ))}
      </AnimatePresence>
    </ol>
  );
}

/** "I agree": a card that fills in and draws its tick when pressed. */
export function AgreeCheck({ checked, onChange, children }: { checked: boolean; onChange: (checked: boolean) => void; children: ReactNode }) {
  return (
    <motion.button
      type="button"
      role="checkbox"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      whileTap={{ scale: 0.98 }}
      className={cn(
        "relative flex w-full items-center gap-3 rounded-2xl border p-3 text-left text-sm font-bold transition-colors",
        checked ? "border-primary/60 bg-primary/10" : "hover:border-primary/40",
      )}
    >
      <motion.span
        animate={checked ? { scale: [1, 1.25, 1] } : { scale: 1 }}
        transition={{ duration: 0.3 }}
        className={cn(
          "grid size-6 shrink-0 place-items-center rounded-lg border-2 transition-colors",
          checked ? "border-primary bg-primary text-primary-foreground" : "border-muted-foreground/40",
        )}
      >
        <AnimatePresence initial={false}>
          {checked && (
            <motion.svg key="tick" viewBox="0 0 24 24" className="size-4" fill="none" stroke="currentColor" strokeWidth={3.5} strokeLinecap="round" strokeLinejoin="round" exit={{ opacity: 0, scale: 0.4 }}>
              <motion.path d="M5 12.5l4.5 4.5L19 7.5" initial={{ pathLength: 0 }} animate={{ pathLength: 1 }} transition={{ duration: 0.25, ease: "easeOut" }} />
            </motion.svg>
          )}
        </AnimatePresence>
      </motion.span>
      <span className="min-w-0 flex-1">{children}</span>
    </motion.button>
  );
}

/**
 * A server's rules. Members who haven't agreed to them yet agree here and can
 * talk straight away; everyone else just reads them.
 */
export function RulesDialog({
  open,
  onOpenChange,
  instanceKey,
  server,
  agree,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  server: Server;
  /** They haven't agreed yet. */
  agree: boolean;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <RulesBody instanceKey={instanceKey} server={server} agree={agree} onDone={() => onOpenChange(false)} />
      </DialogContent>
    </Dialog>
  );
}

function RulesBody({ instanceKey, server, agree, onDone }: { instanceKey: string; server: Server; agree: boolean; onDone: () => void }) {
  const [rules, setRules] = useState<string[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [checked, setChecked] = useState(false);
  const accept = useAction(agreeToRules);
  const nudge = useAnimationControls();
  // Only while it's open: agreeing elsewhere (another tab) mustn't turn this into the reading view mid-way.
  const [agreeing] = useState(agree);
  const { t } = useI18n();

  useEffect(() => {
    let cancelled = false;
    run(getJoinForm(instanceKey, server.id)).then(
      (form) => !cancelled && setRules(form.rules),
      (e: FuwaError) => !cancelled && setProblem(e.message),
    );
    return () => {
      cancelled = true;
    };
  }, [instanceKey, server.id]);

  async function submit() {
    if (!checked) {
      void nudge.start({ x: [0, -8, 8, -5, 5, 0], transition: { duration: 0.4 } });
      return;
    }
    if ((await accept.go(instanceKey, server.id)) === undefined) return;
    toast(t("join.rules.welcomeToast", { server: server.name }));
    onDone();
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="-mx-6 -mt-6 flex items-center gap-3 rounded-t-3xl bg-gradient-to-b from-primary/15 to-transparent px-6 pt-6 pb-1">
        <motion.span initial={{ scale: 0.5, rotate: -14 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 380, damping: 13 }} className="relative shrink-0">
          <ServerIcon server={server} active className="size-12 text-base" />
          <motion.span
            initial={{ scale: 0, rotate: -40 }}
            animate={{ scale: 1, rotate: 0 }}
            transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.25 }}
            className="absolute -right-1.5 -bottom-1.5 grid size-6 place-items-center rounded-full bg-primary text-primary-foreground ring-2 ring-card"
          >
            <ScrollTextIcon className="size-3.5" />
          </motion.span>
        </motion.span>
        <div className="min-w-0 flex-1 pt-1">
          <DialogHeader
            title={agreeing ? t("join.rules.beforeYouTalkIn", { server: server.name }) : t("join.rules.title", { server: server.name })}
            description={agreeing ? t("join.rules.agreeNote") : t("join.rules.readNote")}
          />
        </div>
      </div>

      {problem ? (
        <p className="text-sm text-muted-foreground first-letter:uppercase">{problem}</p>
      ) : rules === null ? (
        <div className="flex flex-col gap-2">
          {[80, 60, 70].map((w, n) => (
            <div key={n} className="shimmer h-12 rounded-2xl" style={{ width: `${w + 20}%` }} />
          ))}
        </div>
      ) : rules.length === 0 ? (
        <p className="rounded-2xl bg-muted/50 p-3 text-sm text-muted-foreground">{agreeing ? t("join.rules.noneTalk") : t("join.rules.none")}</p>
      ) : (
        <RulesList rules={rules} className="scroll-thin max-h-[45svh] overflow-y-auto pr-1" />
      )}

      {agreeing && rules !== null && rules.length > 0 && (
        <motion.div animate={nudge} className="flex flex-col gap-3">
          <AgreeCheck checked={checked} onChange={setChecked}>
            {t("join.rules.agree")}
          </AgreeCheck>
          {accept.error && <p className="text-sm text-destructive first-letter:uppercase">{accept.error}</p>}
          <Button
            size="lg"
            onClick={() => void submit()}
            disabled={accept.pending}
            className={cn("h-11 rounded-xl font-bold transition-opacity", checked ? "btn" : "opacity-60")}
          >
            <AnimatePresence mode="wait" initial={false}>
              <motion.span
                key={accept.pending ? "busy" : "agree"}
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -6 }}
                transition={SPRING}
                className="flex items-center gap-2"
              >
                {accept.pending ? <LoaderCircleIcon className="animate-spin" /> : <PartyPopperIcon />}
                {t("join.rules.agreeAndTalk")}
              </motion.span>
            </AnimatePresence>
          </Button>
        </motion.div>
      )}
      {(!agreeing || (rules !== null && rules.length === 0)) && (
        <Button variant="outline" onClick={onDone} className="h-10 rounded-xl font-bold">
          {agreeing ? t("join.rules.startTalking") : t("common.close")}
        </Button>
      )}
    </div>
  );
}
