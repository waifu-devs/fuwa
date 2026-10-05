import { AlignLeftIcon, GripVerticalIcon, MinusIcon, PlusIcon, ScrollTextIcon, Trash2Icon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion, Reorder, useDragControls } from "motion/react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { getJoinForm, run, setJoinForm, type QuestionDraft } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction } from "@/fuwa/hooks";
import { AgreeCheck, RulesList } from "@/components/join/Rules";
import { Count } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { SaveBar, WithPreview } from "@/components/settings/controls";
import { Segmented } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { useI18n } from "@/i18n/react";
import type { Key } from "@/i18n/i18n";
import { cn } from "@/lib/utils";

/** As on the server. */
const MAX_RULES = 16;
const RULE_MAX = 300;
const MAX_QUESTIONS = 5;
const PROMPT_MAX = 200;

type Rule = { id: number; text: string };
type Question = QuestionDraft & { id: number };

let nextId = 1;
const rulesOf = (list: string[]): Rule[] => list.map((text) => ({ id: nextId++, text }));
const questionsOf = (list: QuestionDraft[]): Question[] => list.map((q) => ({ prompt: q.prompt, paragraph: q.paragraph, required: q.required, id: nextId++ }));

/** Rules to start from, as catalog keys; picked, one becomes the rule's text in the app's language. */
const STARTERS: readonly Key[] = ["serversettings.joinForm.starterKind", "serversettings.joinForm.starterSafe", "serversettings.joinForm.starterSpam"];

/**
 * The rules new members agree to before they talk, and the questions people
 * answer when they apply. Rules drag into order; each question is a line or a
 * few paragraphs, and can be optional.
 */
export function JoinFormEditor({ instanceKey, server, onOpenAccess }: { instanceKey: string; server: Server; onOpenAccess: () => void }) {
  const { t } = useI18n();
  const [saved, setSaved] = useState<{ rules: string[]; questions: QuestionDraft[] } | null>(null);
  const [rules, setRules] = useState<Rule[]>([]);
  const [questions, setQuestions] = useState<Question[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  const save = useAction(setJoinForm);
  const focusLast = useRef<"rule" | "question" | null>(null);

  function load(form: { rules: string[]; questions: QuestionDraft[] }) {
    const clean = { rules: [...form.rules], questions: form.questions.map((q) => ({ prompt: q.prompt, paragraph: q.paragraph, required: q.required })) };
    setSaved(clean);
    setRules(rulesOf(clean.rules));
    setQuestions(questionsOf(clean.questions));
  }

  useEffect(() => {
    run(getJoinForm(instanceKey, server.id)).then(load, (e: FuwaError) => setProblem(e.message));
  }, [instanceKey, server.id]);

  const draft = useMemo(
    () => ({
      rules: rules.map((r) => r.text.trim()).filter(Boolean),
      questions: questions.filter((q) => q.prompt.trim()).map((q) => ({ prompt: q.prompt.trim(), paragraph: q.paragraph, required: q.required })),
    }),
    [rules, questions],
  );
  // Starting points, while the rules are only ever these.
  const starters = STARTERS.map((key) => t(key));
  const startersSet = new Set(starters);
  const ideas = rules.every((r) => startersSet.has(r.text) || !r.text.trim()) ? starters.filter((text) => !rules.some((r) => r.text === text)) : [];
  const changes = saved
    ? Number(JSON.stringify(draft.rules) !== JSON.stringify(saved.rules)) + Number(JSON.stringify(draft.questions) !== JSON.stringify(saved.questions))
    : 0;

  async function submit() {
    const form = await save.go(instanceKey, server.id, draft.rules, draft.questions);
    if (form) load(form);
  }

  if (problem) return <p className="text-sm text-muted-foreground first-letter:uppercase">{problem}</p>;
  if (!saved) return <div className="flex flex-col gap-3">{[0, 1, 2].map((n) => <div key={n} className="shimmer h-14 rounded-2xl" />)}</div>;

  return (
    <WithPreview preview={<FormPreview rules={draft.rules} questions={server.applications ? draft.questions : []} />}>
      <div className="flex flex-col">
        <section data-setting="rules" className="flex flex-col gap-3 border-b border-border/70 pb-6">
          <span className="flex items-end justify-between gap-3">
            <span>
              <span className="block font-extrabold">{t("serversettings.nav.rules")}</span>
              <span className="block text-sm text-muted-foreground">{t("serversettings.joinForm.rulesHint")}</span>
            </span>
            <span className="shrink-0 text-xs font-bold text-muted-foreground tabular-nums">
              <Count value={rules.length} />/{MAX_RULES}
            </span>
          </span>
          <Reorder.Group axis="y" values={rules} onReorder={setRules} className="flex flex-col gap-2">
            <AnimatePresence initial={false}>
              {rules.map((rule, n) => (
                <RuleRow
                  key={rule.id}
                  rule={rule}
                  index={n}
                  autoFocus={focusLast.current === "rule" && n === rules.length - 1}
                  onChange={(text) => setRules((list) => list.map((r) => (r.id === rule.id ? { ...r, text } : r)))}
                  onRemove={() => setRules((list) => list.filter((r) => r.id !== rule.id))}
                />
              ))}
            </AnimatePresence>
          </Reorder.Group>
          <AnimatePresence initial={false}>
            {ideas.length > 0 && (
              <motion.div
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: "auto" }}
                exit={{ opacity: 0, height: 0 }}
                transition={SPRING}
                className="overflow-hidden"
              >
                <div className="flex flex-col gap-2 rounded-2xl border border-dashed p-4">
                  <p className="text-sm text-muted-foreground">
                    {rules.length ? t("serversettings.joinForm.moreStarters") : t("serversettings.joinForm.noRules")}
                  </p>
                  <div className="flex flex-wrap gap-2">
                    <AnimatePresence initial={false} mode="popLayout">
                      {ideas.map((text) => (
                        <motion.button
                          key={text}
                          type="button"
                          layout
                          initial={{ opacity: 0, scale: 0.8 }}
                          animate={{ opacity: 1, scale: 1 }}
                          exit={{ opacity: 0, scale: 0.6, y: -12 }}
                          whileHover={{ y: -2 }}
                          whileTap={{ scale: 0.95 }}
                          transition={SPRING}
                          onClick={() => {
                            const id = nextId++;
                            setRules((list) => [...list, { id, text }]);
                          }}
                          className="flex items-center gap-1 rounded-full border px-3 py-1 text-xs font-bold transition-colors hover:border-primary/50 hover:text-primary"
                        >
                          <PlusIcon className="size-3" /> {text}
                        </motion.button>
                      ))}
                    </AnimatePresence>
                  </div>
                </div>
              </motion.div>
            )}
          </AnimatePresence>
          <Button
            type="button"
            variant="outline"
            disabled={rules.length >= MAX_RULES}
            onClick={() => {
              focusLast.current = "rule";
              const id = nextId++;
              setRules((list) => [...list, { id, text: "" }]);
            }}
            className="group self-start rounded-xl font-bold"
          >
            <PlusIcon className="transition-transform duration-300 group-hover:rotate-90" /> {t("serversettings.joinForm.addRule")}
          </Button>
        </section>

        <section data-setting="questions" className="flex flex-col gap-3 pt-6">
          <span className="flex items-end justify-between gap-3">
            <span>
              <span className="block font-extrabold">{t("serversettings.joinForm.questions")}</span>
              <span className="block text-sm text-muted-foreground">{t("serversettings.joinForm.questionsHint")}</span>
            </span>
            <span className="shrink-0 text-xs font-bold text-muted-foreground tabular-nums">
              <Count value={questions.length} />/{MAX_QUESTIONS}
            </span>
          </span>
          <AnimatePresence initial={false}>
            {!server.applications && (
              <motion.div
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: "auto" }}
                exit={{ opacity: 0, height: 0 }}
                className="flex flex-wrap items-center gap-3 overflow-hidden rounded-2xl bg-muted/60 p-3 text-sm"
              >
                <span className="min-w-0 flex-1 text-muted-foreground">{t("serversettings.joinForm.notAsked")}</span>
                <Button type="button" size="sm" variant="outline" onClick={onOpenAccess} className="rounded-xl font-bold">
                  {t("serversettings.joinForm.turnOnApply")}
                </Button>
              </motion.div>
            )}
          </AnimatePresence>
          <AnimatePresence initial={false} mode="popLayout">
            {questions.map((q, n) => (
              <QuestionCard
                key={q.id}
                question={q}
                index={n}
                autoFocus={focusLast.current === "question" && n === questions.length - 1}
                onChange={(patch) => setQuestions((list) => list.map((x) => (x.id === q.id ? { ...x, ...patch } : x)))}
                onRemove={() => setQuestions((list) => list.filter((x) => x.id !== q.id))}
              />
            ))}
          </AnimatePresence>
          <Button
            type="button"
            variant="outline"
            disabled={questions.length >= MAX_QUESTIONS}
            onClick={() => {
              focusLast.current = "question";
              const id = nextId++;
              setQuestions((list) => [...list, { id, prompt: "", paragraph: false, required: true }]);
            }}
            className="group self-start rounded-xl font-bold"
          >
            <PlusIcon className="transition-transform duration-300 group-hover:rotate-90" /> {t("serversettings.joinForm.addQuestion")}
          </Button>
        </section>
      </div>
      <SaveBar
        count={changes}
        saving={save.pending}
        error={save.error}
        onSave={() => void submit()}
        onDiscard={() => {
          setRules(rulesOf(saved.rules));
          setQuestions(questionsOf(saved.questions));
          save.setError(null);
        }}
      />
    </WithPreview>
  );
}

function RuleRow({
  rule,
  index,
  autoFocus,
  onChange,
  onRemove,
}: {
  rule: Rule;
  index: number;
  autoFocus: boolean;
  onChange: (text: string) => void;
  onRemove: () => void;
}) {
  const { t } = useI18n();
  const drag = useDragControls();
  return (
    <Reorder.Item
      value={rule}
      dragListener={false}
      dragControls={drag}
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: "auto" }}
      exit={{ opacity: 0, height: 0, transition: { duration: 0.2 } }}
      whileDrag={{ scale: 1.02, boxShadow: "0 12px 30px -12px rgb(0 0 0 / 0.35)", zIndex: 5 }}
      transition={SPRING}
      className="relative rounded-2xl bg-card"
    >
      <div className="group flex items-center gap-2 rounded-2xl border bg-background/50 p-1.5 pl-1 transition-colors focus-within:border-primary/50">
        <button
          type="button"
          aria-label={t("serversettings.shared.dragToReorder")}
          onPointerDown={(e) => drag.start(e)}
          className="grid h-8 w-6 shrink-0 cursor-grab touch-none place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground active:cursor-grabbing"
        >
          <GripVerticalIcon className="size-4" />
        </button>
        <motion.span
          key={index}
          initial={{ scale: 0.6 }}
          animate={{ scale: 1 }}
          transition={{ type: "spring", stiffness: 600, damping: 18 }}
          className="grid size-6 shrink-0 place-items-center rounded-full bg-primary/15 text-xs font-extrabold text-primary tabular-nums"
        >
          {index + 1}
        </motion.span>
        <Input
          aria-label={t("serversettings.joinForm.rule", { index: index + 1 })}
          autoFocus={autoFocus}
          value={rule.text}
          maxLength={RULE_MAX}
          placeholder={t("serversettings.joinForm.writeRule")}
          onChange={(e) => onChange(e.target.value)}
          className="h-9 flex-1 rounded-lg border-0 bg-transparent px-1.5 shadow-none focus-visible:ring-0"
        />
        <button
          type="button"
          aria-label={t("serversettings.joinForm.removeRule", { index: index + 1 })}
          onClick={onRemove}
          className="grid size-8 shrink-0 place-items-center rounded-lg text-muted-foreground opacity-60 transition hover:bg-destructive/10 hover:text-destructive group-hover:opacity-100"
        >
          <XIcon className="size-4" />
        </button>
      </div>
    </Reorder.Item>
  );
}

function QuestionCard({
  question: q,
  index,
  autoFocus,
  onChange,
  onRemove,
}: {
  question: Question;
  index: number;
  autoFocus: boolean;
  onChange: (patch: Partial<QuestionDraft>) => void;
  onRemove: () => void;
}) {
  const { t } = useI18n();
  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 12, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.95, transition: { duration: 0.2 } }}
      transition={SPRING}
      className="flex flex-col gap-3 rounded-2xl border bg-background/50 p-3 transition-colors focus-within:border-primary/50"
    >
      <div className="flex items-center gap-2">
        <span className="shrink-0 text-xs font-extrabold text-muted-foreground uppercase">{t("serversettings.joinForm.question", { index: index + 1 })}</span>
        <button
          type="button"
          aria-label={t("serversettings.joinForm.removeQuestion", { index: index + 1 })}
          onClick={onRemove}
          className="ml-auto grid size-8 place-items-center rounded-lg text-muted-foreground transition hover:bg-destructive/10 hover:text-destructive"
        >
          <Trash2Icon className="size-4" />
        </button>
      </div>
      <Input
        aria-label={t("serversettings.joinForm.question", { index: index + 1 })}
        autoFocus={autoFocus}
        value={q.prompt}
        maxLength={PROMPT_MAX}
        placeholder={t("serversettings.joinForm.questionPlaceholder")}
        onChange={(e) => onChange({ prompt: e.target.value })}
        className="h-10 rounded-xl"
      />
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <Segmented
          label={t("serversettings.joinForm.answerLength")}
          value={q.paragraph ? "paragraph" : "line"}
          onChange={(v) => onChange({ paragraph: v === "paragraph" })}
          options={[
            { value: "line", label: t("serversettings.joinForm.line"), icon: <MinusIcon className="size-3.5" /> },
            { value: "paragraph", label: t("serversettings.joinForm.paragraphs"), icon: <AlignLeftIcon className="size-3.5" /> },
          ]}
        />
        <label className="ml-auto flex cursor-pointer items-center gap-2 text-sm font-bold">
          {t("serversettings.joinForm.required")} <Switch checked={q.required} onCheckedChange={(required) => onChange({ required })} />
        </label>
      </div>
    </motion.div>
  );
}

/** The rules and questions as people will see them on their way in. */
function FormPreview({ rules, questions }: { rules: string[]; questions: QuestionDraft[] }) {
  const { t } = useI18n();
  const [agreed, setAgreed] = useState(false);
  return (
    <div className="flex flex-col gap-3 rounded-3xl border bg-card p-4 shadow-lg">
      <p className="flex items-center gap-1.5 text-xs font-bold text-muted-foreground">
        <ScrollTextIcon className="size-3.5" /> {questions.length ? t("serversettings.joinForm.previewApplying") : t("serversettings.joinForm.previewFirstLook")}
      </p>
      {rules.length ? (
        <RulesList rules={rules} className="max-h-72 overflow-y-auto [&_li]:p-2 [&_li]:text-xs" />
      ) : (
        <p className="rounded-2xl bg-muted/50 p-3 text-xs text-muted-foreground">{t("serversettings.joinForm.previewNoRules")}</p>
      )}
      <AnimatePresence initial={false}>
        {questions.map((q, n) => (
          <motion.div
            key={`${n}:${q.prompt}`}
            layout
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            className="flex flex-col gap-1"
          >
            <span className="text-xs font-bold break-words">
              {q.prompt}
              {q.required && <span className="text-destructive"> *</span>}
            </span>
            <span className={cn("rounded-lg border bg-background/60", q.paragraph ? "h-12" : "h-7")} />
          </motion.div>
        ))}
      </AnimatePresence>
      {rules.length > 0 && (
        <AgreeCheck checked={agreed} onChange={setAgreed}>
          <span className="text-xs">{t("join.rules.agree")}</span>
        </AgreeCheck>
      )}
    </div>
  );
}
