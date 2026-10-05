import * as DialogPrimitive from "@radix-ui/react-dialog";
import { ClipboardPenIcon, LoaderCircleIcon, SendIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls, useScroll } from "motion/react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import type { JoinForm, Server } from "@/gen/fuwa/v1/types_pb";
import { applyToJoin, getJoinForm, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction } from "@/fuwa/hooks";
import { ApplicationCard } from "@/components/join/ApplicationStatus";
import { BannerHero } from "@/components/join/Banner";
import { AgreeCheck, RulesList } from "@/components/join/Rules";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

/** As on the server. */
const LINE_MAX = 300;
const PARAGRAPH_MAX = 1000;

/**
 * Asking to join a server that lets people in by hand: its rules to agree
 * to, its questions to answer, then a note that someone will look it over.
 */
export function ApplyDialog({
  open,
  onOpenChange,
  instanceKey,
  server,
  inviteCode,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  server: Server;
  inviteCode: string;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="overflow-x-hidden">
        <ApplyBody instanceKey={instanceKey} server={server} inviteCode={inviteCode} onDone={() => onOpenChange(false)} />
      </DialogContent>
    </Dialog>
  );
}

function ApplyBody({ instanceKey, server, inviteCode, onDone }: { instanceKey: string; server: Server; inviteCode: string; onDone: () => void }) {
  const [form, setForm] = useState<JoinForm | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [answers, setAnswers] = useState<string[]>([]);
  const [agreed, setAgreed] = useState(false);
  const [missing, setMissing] = useState<number[]>([]);
  const [sent, setSent] = useState(false);
  const [reload, setReload] = useState(0);
  const apply = useAction(applyToJoin);
  const nudge = useAnimationControls();
  const scroller = useRef<HTMLFormElement>(null);
  const { scrollY } = useScroll({ container: scroller });
  const { t } = useI18n();

  useEffect(() => {
    let cancelled = false;
    run(getJoinForm(instanceKey, server.id, inviteCode)).then(
      (f) => {
        if (cancelled) return;
        setForm(f);
        // Keep what they wrote for questions that are still asked.
        setAnswers((before) => f.questions.map((_, n) => before[n] ?? ""));
      },
      (e: FuwaError) => !cancelled && setProblem(e.message),
    );
    return () => {
      cancelled = true;
    };
  }, [instanceKey, server.id, inviteCode, reload]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!form) return;
    const empty = form.questions.flatMap((q, n) => (q.required && !answers[n]?.trim() ? [n] : []));
    setMissing(empty);
    if (empty.length || (form.rules.length > 0 && !agreed)) {
      void nudge.start({ x: [0, -8, 8, -5, 5, 0], transition: { duration: 0.4 } });
      return;
    }
    const done = await apply.go(
      instanceKey,
      server,
      inviteCode,
      form.questions.map((q, n) => ({ question: q.prompt, answer: answers[n]?.trim() ?? "" })),
    );
    if (done) setSent(true);
  }

  // The questions changed while they were answering: show the new ones.
  useEffect(() => {
    if (apply.error && /questions just changed/.test(apply.error)) setReload((n) => n + 1);
  }, [apply.error]);

  if (sent)
    return (
      <motion.div initial={{ opacity: 0, scale: 0.97 }} animate={{ opacity: 1, scale: 1 }} transition={SPRING}>
        <ApplicationCard instanceKey={instanceKey} server={server} onDone={onDone} />
      </motion.div>
    );

  return (
    <form ref={scroller} onSubmit={submit} className="scroll-thin -m-6 flex max-h-[92svh] flex-col gap-4 overflow-y-auto p-6">
      <BannerHero server={server} bleed eyebrow={t("join.apply")} badge={<ClipboardPenIcon className="size-3.5" />} scrollY={scrollY}>
        <DialogPrimitive.Title className="sr-only">{t("join.applyDialog.title", { server: server.name })}</DialogPrimitive.Title>
        <DialogPrimitive.Description className="mt-1 text-sm text-muted-foreground">{t("join.applyDialog.description")}</DialogPrimitive.Description>
      </BannerHero>

      {problem ? (
        <p className="text-sm text-muted-foreground first-letter:uppercase">{problem}</p>
      ) : !form ? (
        <div className="flex flex-col gap-3">
          <div className="shimmer h-12 rounded-2xl" />
          <div className="shimmer h-4 w-1/3 rounded-lg" />
          <div className="shimmer h-11 rounded-xl" />
        </div>
      ) : (
        <>
          {form.rules.length > 0 && (
            <section className="flex flex-col gap-2">
              <h3 className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("join.applyDialog.rules")}</h3>
              <RulesList rules={form.rules} className="scroll-thin max-h-56 overflow-y-auto pr-1" />
            </section>
          )}
          {form.questions.length > 0 && (
            <section className="flex flex-col gap-3">
              <h3 className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("join.applyDialog.questions")}</h3>
              {form.questions.map((q, n) => {
                const max = q.paragraph ? PARAGRAPH_MAX : LINE_MAX;
                const value = answers[n] ?? "";
                const bad = missing.includes(n) && !value.trim();
                const props = {
                  id: `apply-${n}`,
                  value,
                  maxLength: max,
                  "aria-invalid": bad,
                  onChange: (e: { target: { value: string } }) => setAnswers((list) => list.map((a, i) => (i === n ? e.target.value : a))),
                };
                return (
                  <motion.div
                    key={`${n}:${q.prompt}`}
                    initial={{ opacity: 0, y: 8 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ ...SPRING, delay: 0.05 * n }}
                    className="flex flex-col gap-1.5"
                  >
                    <label htmlFor={`apply-${n}`} className="flex items-baseline justify-between gap-2 text-sm font-bold">
                      <span className="min-w-0 break-words">
                        {q.prompt}
                        {q.required && <span className="text-destructive"> *</span>}
                      </span>
                      {!q.required && <span className="shrink-0 text-xs font-normal text-muted-foreground">{t("join.applyDialog.optional")}</span>}
                    </label>
                    {q.paragraph ? (
                      <Textarea rows={3} {...props} className={cn("rounded-xl", bad && "border-destructive")} />
                    ) : (
                      <Input {...props} className={cn("h-11 rounded-xl", bad && "border-destructive")} />
                    )}
                    <AnimatePresence initial={false}>
                      {(bad || value.length > max - 100) && (
                        <motion.p
                          initial={{ opacity: 0, height: 0 }}
                          animate={{ opacity: 1, height: "auto" }}
                          exit={{ opacity: 0, height: 0 }}
                          className={cn("text-xs", bad ? "text-destructive" : "text-muted-foreground")}
                        >
                          {bad ? t("join.applyDialog.needsAnswer") : t("join.applyDialog.charactersLeft", { count: max - value.length })}
                        </motion.p>
                      )}
                    </AnimatePresence>
                  </motion.div>
                );
              })}
            </section>
          )}
          <motion.div animate={nudge} className="flex flex-col gap-3">
            {form.rules.length > 0 && (
              <AgreeCheck checked={agreed} onChange={setAgreed}>
                {t("join.rules.agree")}
              </AgreeCheck>
            )}
            {apply.error && <p className="text-sm text-destructive first-letter:uppercase">{apply.error}</p>}
            <Button type="submit" size="lg" disabled={apply.pending} className="btn group h-11 rounded-xl font-bold">
              {apply.pending ? <LoaderCircleIcon className="animate-spin" /> : <SendIcon className="transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5" />}
              {t("join.applyDialog.send")}
            </Button>
          </motion.div>
        </>
      )}
    </form>
  );
}
