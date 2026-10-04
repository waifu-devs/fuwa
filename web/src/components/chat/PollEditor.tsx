import { EyeIcon, EyeOffIcon, ListChecksIcon, LoaderCircleIcon, PlusIcon, SmilePlusIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import type { Channel } from "@/gen/fuwa/v1/types_pb";
import { sendPoll, type PollDraft } from "@/fuwa/actions";
import { useAction } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { EmojiGlyph } from "@/components/EmojiGlyph";
import { EmojiPicker } from "@/components/EmojiPicker";
import { SPRING, SwapText } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";

const MAX_QUESTION = 300;
const MAX_ANSWER = 55;
const MAX_ANSWERS = 10;

/** How long a poll can run, as the server takes it: hours, 0 for until it's ended. */
const DURATIONS = [
  { hours: 1, label: "1 hour" },
  { hours: 4, label: "4 hours" },
  { hours: 24, label: "1 day" },
  { hours: 72, label: "3 days" },
  { hours: 168, label: "1 week" },
  { hours: 336, label: "2 weeks" },
  { hours: 0, label: "No end" },
] as const;

type Answer = { key: number; text: string; emoji: string };

let nextKey = 0;
const blank = (): Answer => ({ key: nextKey++, text: "", emoji: "" });

/**
 * Where a poll is written: a question, 2 to 10 answers each with an emoji if
 * you like, single or multiple choice, how long it runs, and whether votes
 * are anonymous. Answers slide in and out as they're added and removed.
 */
export function PollEditor({
  open,
  onOpenChange,
  instanceKey,
  serverId,
  channel,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  serverId: string;
  channel: Channel;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader title="Make a poll" description={`Everyone who can see #${channel.name} can vote.`} />
        {/* A fresh form each time it opens. */}
        {open && <PollForm instanceKey={instanceKey} serverId={serverId} channelId={channel.id} onDone={() => onOpenChange(false)} />}
      </DialogContent>
    </Dialog>
  );
}

function PollForm({ instanceKey, serverId, channelId, onDone }: { instanceKey: string; serverId: string; channelId: string; onDone: () => void }) {
  const [question, setQuestion] = useState("");
  const [answers, setAnswers] = useState<Answer[]>(() => [blank(), blank()]);
  const [multiple, setMultiple] = useState(false);
  const [anonymous, setAnonymous] = useState(false);
  const [hours, setHours] = useState(24);
  const send = useAction(sendPoll);
  const emojis = useFuwa((s) => s.instances[instanceKey]?.emojis[serverId]);
  const server = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId));
  const inputs = useRef(new Map<number, HTMLInputElement>());

  const filled = answers.filter((a) => a.text.trim());
  const ready = !!question.trim() && filled.length >= 2 && !send.pending;

  const update = (key: number, patch: Partial<Answer>) => setAnswers((list) => list.map((a) => (a.key === key ? { ...a, ...patch } : a)));
  const focus = (key: number) => requestAnimationFrame(() => inputs.current.get(key)?.focus());

  function add() {
    if (answers.length >= MAX_ANSWERS) return;
    const answer = blank();
    setAnswers((list) => [...list, answer]);
    focus(answer.key);
  }

  function remove(key: number) {
    setAnswers((list) => (list.length > 2 ? list.filter((a) => a.key !== key) : list));
  }

  /** Enter moves to the next answer, adding one at the end while there's room. */
  function onAnswerKey(e: KeyboardEvent<HTMLInputElement>, at: number) {
    if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
    e.preventDefault();
    const next = answers[at + 1];
    if (next) focus(next.key);
    else if (answers[at]?.text.trim()) add();
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!ready) return;
    const draft: PollDraft = { question, answers: filled.map((a) => ({ text: a.text, emoji: a.emoji })), multiple, anonymous, hours };
    if (await send.go(instanceKey, serverId, channelId, draft)) onDone();
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-5">
      <label className="flex flex-col gap-2">
        <span className="flex items-baseline justify-between text-sm font-bold">
          Question
          <span className={cn("text-xs font-normal text-muted-foreground tabular-nums", question.length > MAX_QUESTION - 30 && "text-amber-600 dark:text-amber-400")}>
            {MAX_QUESTION - question.length}
          </span>
        </span>
        <Input
          autoFocus
          value={question}
          maxLength={MAX_QUESTION}
          onChange={(e) => setQuestion(e.target.value)}
          placeholder="What should we play tonight?"
          className="h-11 rounded-xl text-[0.95rem] font-bold"
        />
      </label>

      <fieldset className="flex flex-col gap-2">
        <legend className="mb-2 flex w-full items-baseline justify-between text-sm font-bold">
          Answers
          <span className="text-xs font-normal text-muted-foreground tabular-nums">
            {answers.length} of {MAX_ANSWERS}
          </span>
        </legend>
        <AnimatePresence initial={false}>
          {answers.map((answer, n) => (
            <motion.div
              key={answer.key}
              layout="position"
              initial={{ opacity: 0, y: -8, scale: 0.97 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, x: 24, scale: 0.95, transition: { duration: 0.16 } }}
              transition={SPRING}
              className="flex items-center gap-2"
            >
              <EmojiPicker
                emojis={emojis}
                server={server}
                placement="bottom-start"
                onPick={(emoji) => update(answer.key, { emoji: emoji.text })}
              >
                {(pickerOpen) => (
                  <motion.button
                    type="button"
                    aria-label={answer.emoji ? `Answer ${n + 1}'s emoji` : `Add an emoji to answer ${n + 1}`}
                    whileHover={{ scale: 1.08, rotate: -6 }}
                    whileTap={{ scale: 0.9 }}
                    transition={SPRING}
                    className={cn(
                      "grid size-10 shrink-0 place-items-center rounded-xl border bg-muted/40 text-muted-foreground transition-colors hover:text-primary",
                      pickerOpen && "border-primary/50 text-primary",
                    )}
                  >
                    <AnimatePresence mode="popLayout" initial={false}>
                      <motion.span
                        key={answer.emoji || "none"}
                        initial={{ scale: 0.3, rotate: -30, opacity: 0 }}
                        animate={{ scale: 1, rotate: 0, opacity: 1 }}
                        exit={{ scale: 0.3, opacity: 0 }}
                        transition={{ type: "spring", stiffness: 600, damping: 18 }}
                        className="grid place-items-center"
                      >
                        {answer.emoji ? <EmojiGlyph value={answer.emoji} emojis={emojis} className="size-5 text-lg" /> : <SmilePlusIcon className="size-[18px]" />}
                      </motion.span>
                    </AnimatePresence>
                  </motion.button>
                )}
              </EmojiPicker>
              <Input
                ref={(el) => {
                  if (el) inputs.current.set(answer.key, el);
                  else inputs.current.delete(answer.key);
                }}
                value={answer.text}
                maxLength={MAX_ANSWER}
                onChange={(e) => update(answer.key, { text: e.target.value })}
                onKeyDown={(e) => onAnswerKey(e, n)}
                placeholder={`Answer ${n + 1}`}
                aria-label={`Answer ${n + 1}`}
                className="h-10 flex-1 rounded-xl"
              />
              <motion.button
                type="button"
                aria-label={`Remove answer ${n + 1}`}
                onClick={() => remove(answer.key)}
                disabled={answers.length <= 2}
                whileHover={answers.length > 2 ? { scale: 1.1, rotate: 90 } : undefined}
                whileTap={{ scale: 0.85 }}
                transition={SPRING}
                className="grid size-8 shrink-0 place-items-center rounded-full text-muted-foreground transition-opacity hover:bg-destructive/10 hover:text-destructive disabled:pointer-events-none disabled:opacity-0"
              >
                <XIcon className="size-4" />
              </motion.button>
            </motion.div>
          ))}
        </AnimatePresence>
        <AnimatePresence initial={false}>
          {answers.length < MAX_ANSWERS && (
            <motion.button
              key="add"
              type="button"
              layout="position"
              onClick={add}
              initial={{ opacity: 0, scale: 0.95 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.95 }}
              whileTap={{ scale: 0.97 }}
              transition={SPRING}
              className="group flex h-10 items-center justify-center gap-1.5 rounded-xl border border-dashed text-sm font-bold text-muted-foreground transition-colors hover:border-primary/50 hover:text-primary"
            >
              <PlusIcon className="size-4 transition-transform group-hover:rotate-90" />
              Add an answer
            </motion.button>
          )}
        </AnimatePresence>
      </fieldset>

      <div className="flex flex-col gap-2">
        <span className="text-sm font-bold">Runs for</span>
        <div role="radiogroup" aria-label="How long the poll runs" className="flex flex-wrap gap-1.5">
          {DURATIONS.map((d) => {
            const active = hours === d.hours;
            return (
              <motion.button
                key={d.hours}
                type="button"
                role="radio"
                aria-checked={active}
                onClick={() => setHours(d.hours)}
                whileTap={{ scale: 0.94 }}
                className={cn(
                  "relative rounded-full border px-3 py-1.5 text-xs font-bold transition-colors",
                  active ? "border-transparent text-primary-foreground" : "text-muted-foreground hover:border-primary/40 hover:text-foreground",
                )}
              >
                {active && <motion.span layoutId="poll-duration" transition={SPRING} className="absolute inset-0 rounded-full bg-primary" />}
                <span className="relative">{d.label}</span>
              </motion.button>
            );
          })}
        </div>
      </div>

      <div className="flex flex-col divide-y rounded-2xl border">
        <Toggle
          id="poll-multiple"
          icon={<ListChecksIcon className="size-[18px]" />}
          title="Pick more than one"
          about={multiple ? "People can choose as many answers as they like." : "People choose one answer."}
          checked={multiple}
          onChange={setMultiple}
        />
        <Toggle
          id="poll-anonymous"
          icon={anonymous ? <EyeOffIcon className="size-[18px]" /> : <EyeIcon className="size-[18px]" />}
          title="Anonymous votes"
          about={
            anonymous
              ? "Nobody can see who voted for what, not even admins. Voters see this before they vote."
              : "Everyone in the channel can see who voted for what. Voters see this before they vote."
          }
          checked={anonymous}
          onChange={setAnonymous}
        />
      </div>

      <AnimatePresence>
        {send.error && (
          <motion.p
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            role="alert"
            className="rounded-xl bg-destructive/10 px-3 py-2 text-sm font-bold text-destructive"
          >
            {send.error}
          </motion.p>
        )}
      </AnimatePresence>

      <Button type="submit" disabled={!ready} className="btn h-11 rounded-xl font-bold">
        {send.pending ? <LoaderCircleIcon className="size-4 animate-spin" /> : null}
        <SwapText>{filled.length < 2 ? "Add at least two answers" : !question.trim() ? "Ask a question" : "Post poll"}</SwapText>
      </Button>
    </form>
  );
}

function Toggle({
  id,
  icon,
  title,
  about,
  checked,
  onChange,
}: {
  id: string;
  icon: React.ReactNode;
  title: string;
  about: string;
  checked: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <label htmlFor={id} className="flex cursor-pointer items-center gap-3 p-3">
      <span className={cn("grid size-9 shrink-0 place-items-center rounded-xl transition-colors", checked ? "bg-primary/15 text-primary" : "bg-muted text-muted-foreground")}>
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span key={String(checked)} initial={{ scale: 0.4, rotate: -30, opacity: 0 }} animate={{ scale: 1, rotate: 0, opacity: 1 }} transition={SPRING}>
            {icon}
          </motion.span>
        </AnimatePresence>
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-sm font-bold">{title}</span>
        <span className="block text-xs text-muted-foreground">{about}</span>
      </span>
      <Switch id={id} checked={checked} onCheckedChange={onChange} />
    </label>
  );
}
