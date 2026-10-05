import { BarChart3Icon, CheckIcon, EyeIcon, EyeOffIcon, FlagIcon, LoaderCircleIcon, TrophyIcon, UndoIcon, UsersIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useContext, useEffect, useRef, useState, type ReactNode } from "react";
import type { Emoji, Message, Poll, PollAnswer, User } from "@/gen/fuwa/v1/types_pb";
import { endPoll, listPollVoters, run, votePoll } from "@/fuwa/actions";
import { EmojiGlyph } from "@/components/EmojiGlyph";
import { UserAvatar } from "@/components/Icons";
import { CountUp, SPRING } from "@/components/motion";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { type I18n, T, useI18n } from "@/i18n/react";
import { displayName, formatFull, toDate } from "@/lib/format";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { PollPlace, type PollPlaceValue } from "./pollPlace";


/** The bars grow on a soft spring, so a new vote reads as movement rather than a jump. */
const BAR = { type: "spring", stiffness: 140, damping: 24, mass: 0.9 } as const;

/** "47 minutes left", "5 hours left", "3 days left". */
function left(t: I18n["t"], ms: number): string {
  const minutes = Math.max(1, Math.ceil(ms / 60_000));
  if (minutes < 60) return t("chattools.poll.minutesLeft", { count: minutes });
  const hours = Math.round(minutes / 60);
  if (hours < 48) return t("chattools.poll.hoursLeft", { count: hours });
  return t("chattools.poll.daysLeft", { count: Math.round(hours / 24) });
}

/** Now, ticking while a poll counts down to its end, and once more the moment it ends. */
function useNow(endsAt: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!endsAt) return;
    const ms = endsAt - Date.now();
    if (ms <= 0) return;
    const tick = setInterval(() => setNow(Date.now()), ms < 3_600_000 ? 15_000 : 60_000);
    const end = setTimeout(() => setNow(Date.now()), Math.min(ms + 50, 2 ** 31 - 1));
    return () => {
      clearInterval(tick);
      clearTimeout(end);
    };
  }, [endsAt]);
  return now;
}

/**
 * A poll under its message. Answers are buttons: pick one (or several) and
 * the bars grow to everyone's results, your picks marked with a check that
 * pops in. Whether votes are anonymous shows before you vote; an anonymous
 * poll keeps its counts hidden until it ends, so nobody can tell a pick from
 * a count going up. The creator and moderators can end it early; an ended
 * poll keeps its results, the winning answer crowned.
 */
export function PollCard({ message, mine, animate }: { message: Message; mine: boolean; animate: boolean }) {
  const place = useContext(PollPlace);
  const { t } = useI18n();
  const poll = message.poll!;
  const endsAt = poll.endsAt ? toDate(poll.endsAt).getTime() : 0;
  const now = useNow(poll.endedAt ? 0 : endsAt);
  const closed = !!poll.endedAt || (!!endsAt && endsAt <= now);
  const [picking, setPicking] = useState<number[] | null>(null);
  const [peek, setPeek] = useState(false);
  const chosen = picking ?? poll.myAnswerIds;
  const picked = new Set(chosen);
  const voted = poll.myAnswerIds.length > 0;
  // Anonymous polls show counts only once they're over (the server sends none before).
  const hidden = poll.anonymous && !closed;
  const results = !hidden && (voted || closed || peek);
  const canVote = !!place?.canVote && !closed;
  const total = poll.answers.reduce((n, a) => n + Number(a.votes), 0);
  const top = Math.max(0, ...poll.answers.map((a) => Number(a.votes)));

  // Clicks queue up: only the newest pick goes out after the one in flight, so answers never land out of order.
  const want = useRef<number[] | null>(null);
  const sending = useRef(false);
  async function flush() {
    if (!place || sending.current) return;
    sending.current = true;
    try {
      while (want.current) {
        const ids = want.current;
        want.current = null;
        await run(votePoll(place.instanceKey, place.serverId, place.channelId, message.id, ids)).catch((err: Error) => toast(err.message));
      }
    } finally {
      sending.current = false;
      setPicking(null);
    }
  }
  function vote(ids: number[]) {
    setPicking(ids);
    want.current = ids;
    void flush();
  }
  function pick(answer: PollAnswer) {
    if (!canVote) return;
    if (poll.multiple) vote(picked.has(answer.id) ? chosen.filter((id) => id !== answer.id) : [...chosen, answer.id].sort((a, b) => a - b));
    else vote(chosen.length === 1 && chosen[0] === answer.id ? [] : [answer.id]);
  }

  const counting = closed && poll.anonymous && total === 0 && poll.voters > 0n;
  const running = endsAt ? left(t, endsAt - now) : t("chattools.poll.noEnd");
  const status = counting
    ? t("chattools.poll.counting")
    : poll.endedAt
      ? t("chattools.poll.ended", { time: formatFull(toDate(poll.endedAt)) })
      : closed
        ? t("chattools.poll.ended", { time: formatFull(new Date(endsAt)) })
        : hidden
          ? t("chattools.poll.hiddenUntilEnd", { status: running })
          : running;

  return (
    <motion.section
      aria-label={t("chattools.poll.label", { question: poll.question })}
      initial={animate ? { opacity: 0, y: 8, scale: 0.98 } : false}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={SPRING}
      className={cn("poll mt-1 flex max-w-lg flex-col gap-3 rounded-2xl border bg-card/70 p-3 shadow-sm sm:p-4", closed && "ended")}
    >
      <PollHeader poll={poll} closed={closed} />

      <div role={poll.multiple ? "group" : "radiogroup"} aria-label={t("chattools.poll.answers")} className="flex flex-col gap-1.5">
        {poll.answers.map((answer, n) => (
          <AnswerRow
            key={answer.id}
            answer={answer}
            index={n}
            multiple={poll.multiple}
            chosen={picked.has(answer.id)}
            results={results}
            share={total ? Number(answer.votes) / total : 0}
            winner={closed && top > 0 && Number(answer.votes) === top}
            dim={closed && top > 0 && Number(answer.votes) !== top}
            disabled={!canVote}
            emojis={place?.emojis}
            animate={animate}
            onPick={() => pick(answer)}
          />
        ))}
      </div>

      <PollFooter
        poll={poll}
        place={place}
        messageId={message.id}
        status={status}
        busy={!!picking}
        canPeek={!results && !closed && !hidden}
        onPeek={() => setPeek(true)}
        canTakeBack={voted && canVote}
        onTakeBack={() => vote([])}
        canEnd={!closed && (mine || !!place?.moderator)}
        total={total}
      />
    </motion.section>
  );
}

/** What kind of poll it is, shown before you vote: one or many, anonymous or public, and once it's over. */
function PollHeader({ poll, closed }: { poll: Poll; closed: boolean }) {
  const { t } = useI18n();
  return (
    <header className="flex flex-col gap-1.5">
      <div className="flex flex-wrap items-center gap-1.5 text-[0.7rem] font-extrabold text-muted-foreground">
        <span className="inline-flex items-center gap-1 rounded-full bg-primary/12 px-2 py-0.5 text-primary">
          <BarChart3Icon className="size-3" />
          {t("chattools.poll.badge")}
        </span>
        <span className="rounded-full bg-muted px-2 py-0.5">{poll.multiple ? t("chattools.poll.pickAny") : t("chattools.poll.pickOne")}</span>
        <span
          className="inline-flex items-center gap-1 rounded-full bg-muted px-2 py-0.5"
          title={poll.anonymous ? t("chattools.poll.anonymousAbout") : t("chattools.poll.publicAbout")}
        >
          {poll.anonymous ? <EyeOffIcon className="size-3" /> : <EyeIcon className="size-3" />}
          {poll.anonymous ? t("chattools.poll.anonymous") : t("chattools.poll.public")}
        </span>
        <AnimatePresence initial={false}>
          {closed && (
            <motion.span
              key="ended"
              initial={{ opacity: 0, scale: 0.6, x: -6 }}
              animate={{ opacity: 1, scale: 1, x: 0 }}
              exit={{ opacity: 0, scale: 0.6 }}
              transition={{ type: "spring", stiffness: 600, damping: 20 }}
              className="inline-flex items-center gap-1 rounded-full bg-foreground px-2 py-0.5 text-background"
            >
              <FlagIcon className="size-3" />
              {t("chattools.poll.final")}
            </motion.span>
          )}
        </AnimatePresence>
      </div>
      <h3 className="text-[1.02rem] leading-snug font-extrabold break-words">{poll.question}</h3>
    </header>
  );
}

/** How many voted, how long it runs, and what you can do: peek, take your vote back, see who voted, end it. */
function PollFooter({
  poll,
  place,
  messageId,
  status,
  busy,
  canPeek,
  onPeek,
  canTakeBack,
  onTakeBack,
  canEnd,
  total,
}: {
  poll: Poll;
  place: PollPlaceValue | null;
  messageId: string;
  status: string;
  busy: boolean;
  canPeek: boolean;
  onPeek: () => void;
  canTakeBack: boolean;
  onTakeBack: () => void;
  canEnd: boolean;
  total: number;
}) {
  const [voters, setVoters] = useState(false);
  const { t } = useI18n();
  return (
    <footer className="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-muted-foreground">
      <span className="font-bold tabular-nums">
        <T k="chattools.poll.votes" values={{ count: <CountUp value={Number(poll.voters)} /> }} count={Number(poll.voters)} />
      </span>
      <span aria-hidden>·</span>
      <span>{status}</span>
      <span className="ml-auto flex flex-wrap items-center justify-end gap-1">
        {busy && <LoaderCircleIcon aria-label={t("chattools.poll.voting")} className="size-3.5 animate-spin" />}
        {canPeek && (
          <FooterButton onClick={onPeek}>
            <BarChart3Icon className="size-3.5" />
            {t("chattools.poll.showResults")}
          </FooterButton>
        )}
        {canTakeBack && (
          <FooterButton onClick={onTakeBack}>
            <UndoIcon className="size-3.5" />
            {t("chattools.poll.takeBack")}
          </FooterButton>
        )}
        {!poll.anonymous && total > 0 && (
          <FooterButton onClick={() => setVoters(true)}>
            <UsersIcon className="size-3.5" />
            {t("chattools.poll.whoVoted")}
          </FooterButton>
        )}
        {place && <EndButton place={place} messageId={messageId} shown={canEnd} />}
      </span>
      {!poll.anonymous && place && <VotersDialog open={voters} onOpenChange={setVoters} place={place} messageId={messageId} poll={poll} />}
    </footer>
  );
}

/** Ending a poll early asks once more, in place. */
function EndButton({ place, messageId, shown }: { place: PollPlaceValue; messageId: string; shown: boolean }) {
  const [confirm, setConfirm] = useState(false);
  const [ending, setEnding] = useState(false);
  const { t } = useI18n();
  async function end() {
    setEnding(true);
    try {
      await run(endPoll(place.instanceKey, place.serverId, place.channelId, messageId));
    } catch (err) {
      toast((err as Error).message);
    } finally {
      setEnding(false);
      setConfirm(false);
    }
  }
  return (
    <AnimatePresence mode="popLayout" initial={false}>
      {shown && confirm && (
        <motion.span
          key="confirm"
          initial={{ opacity: 0, x: 8 }}
          animate={{ opacity: 1, x: 0 }}
          exit={{ opacity: 0, x: 8 }}
          transition={SPRING}
          className="flex items-center gap-1"
        >
          <span className="font-bold text-destructive">{t("chattools.poll.endNow")}</span>
          <FooterButton danger onClick={end} label={t("chattools.poll.end")}>
            {ending ? <LoaderCircleIcon className="size-3.5 animate-spin" /> : <CheckIcon className="size-3.5" />}
          </FooterButton>
          <FooterButton onClick={() => setConfirm(false)} label={t("chattools.poll.keepRunning")}>
            <XIcon className="size-3.5" />
          </FooterButton>
        </motion.span>
      )}
      {shown && !confirm && (
        <motion.span key="end" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
          <FooterButton onClick={() => setConfirm(true)}>
            <FlagIcon className="size-3.5" />
            {t("chattools.poll.end")}
          </FooterButton>
        </motion.span>
      )}
    </AnimatePresence>
  );
}

function FooterButton({ onClick, danger, label, children }: { onClick: () => void; danger?: boolean; label?: string; children: ReactNode }) {
  return (
    <motion.button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      whileTap={{ scale: 0.92 }}
      className={cn(
        "inline-flex items-center gap-1 whitespace-nowrap rounded-lg px-2 py-1 font-bold transition-colors",
        danger ? "text-destructive hover:bg-destructive/10" : "hover:bg-muted hover:text-foreground",
      )}
    >
      {children}
    </motion.button>
  );
}

function AnswerRow({
  answer,
  index,
  multiple,
  chosen,
  results,
  share,
  winner,
  dim,
  disabled,
  emojis,
  animate,
  onPick,
}: {
  answer: PollAnswer;
  index: number;
  multiple: boolean;
  chosen: boolean;
  results: boolean;
  share: number;
  winner: boolean;
  dim: boolean;
  disabled: boolean;
  emojis: Emoji[] | undefined;
  animate: boolean;
  onPick: () => void;
}) {
  const percent = Math.round(share * 100);
  const { t } = useI18n();
  return (
    <motion.button
      type="button"
      role={multiple ? "checkbox" : "radio"}
      aria-checked={chosen}
      aria-disabled={disabled}
      onClick={onPick}
      initial={animate ? { opacity: 0, x: -8 } : false}
      animate={{ opacity: dim ? 0.62 : 1, x: 0 }}
      whileHover={disabled ? undefined : { x: 2 }}
      whileTap={disabled ? undefined : { scale: 0.985 }}
      transition={{ ...SPRING, delay: animate ? 0.05 + index * 0.04 : 0 }}
      className={cn(
        "relative isolate overflow-hidden rounded-xl border px-3 py-2 text-left text-sm transition-colors",
        chosen ? "border-primary/60" : "border-border",
        disabled ? "cursor-default" : "hover:border-primary/40",
      )}
    >
      {/* The result bar: a full-width fill scaled from the left, so it moves on the compositor. */}
      <motion.span
        aria-hidden
        initial={false}
        animate={{ scaleX: results ? share : 0 }}
        transition={BAR}
        className={cn("absolute inset-0 -z-10 origin-left", winner ? "bg-primary/30" : chosen ? "bg-primary/22" : "bg-foreground/8")}
      />
      {/* A ring that flashes out when your pick lands. */}
      <AnimatePresence>
        {chosen && (
          <motion.span
            key="flash"
            aria-hidden
            initial={{ opacity: 0.9, scale: 1 }}
            animate={{ opacity: 0, scale: 1.04 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.55, ease: "easeOut" }}
            className="pointer-events-none absolute inset-0 rounded-xl ring-2 ring-primary"
          />
        )}
      </AnimatePresence>
      <span className="flex items-center gap-2.5">
        <span
          className={cn(
            "grid size-[18px] shrink-0 place-items-center border-2 transition-colors",
            multiple ? "rounded-md" : "rounded-full",
            chosen ? "border-primary bg-primary text-primary-foreground" : "border-muted-foreground/40",
          )}
        >
          <AnimatePresence initial={false}>
            {chosen && (
              <motion.span
                key="check"
                initial={{ scale: 0, rotate: -45 }}
                animate={{ scale: 1, rotate: 0 }}
                exit={{ scale: 0 }}
                transition={{ type: "spring", stiffness: 700, damping: 18 }}
                className="grid place-items-center"
              >
                <CheckIcon className="size-3" strokeWidth={3.5} />
              </motion.span>
            )}
          </AnimatePresence>
        </span>
        {answer.emoji && <EmojiGlyph value={answer.emoji} emojis={emojis} className="size-5 shrink-0 text-base" />}
        <span className="min-w-0 flex-1 font-bold break-words">{answer.text}</span>
        <AnimatePresence initial={false}>
          {winner && (
            <motion.span
              key="trophy"
              initial={{ scale: 0, rotate: -40, y: 4 }}
              animate={{ scale: 1, rotate: [0, -12, 8, 0], y: 0 }}
              transition={{ ...SPRING, rotate: { duration: 0.6, delay: 0.15 } }}
              className="text-amber-500"
              aria-label={t("chattools.poll.mostVotes")}
            >
              <TrophyIcon className="size-4" />
            </motion.span>
          )}
        </AnimatePresence>
        <Numbers shown={results} votes={answer.votes} percent={percent} />
      </span>
    </motion.button>
  );
}

/** An answer's votes and share, sliding in once results show. */
function Numbers({ shown, votes, percent }: { shown: boolean; votes: bigint; percent: number }) {
  const { t, number } = useI18n();
  return (
    <AnimatePresence initial={false}>
      {shown && (
        <motion.span
          key="numbers"
          initial={{ opacity: 0, x: 8 }}
          animate={{ opacity: 1, x: 0 }}
          exit={{ opacity: 0, x: 8 }}
          transition={SPRING}
          className="flex shrink-0 items-baseline gap-1.5 tabular-nums"
          title={t("chattools.poll.votes", { count: Number(votes) })}
        >
          <span className="text-xs text-muted-foreground">
            <CountUp value={Number(votes)} />
          </span>
          <span className="w-9 text-right text-xs font-extrabold">
            <CountUp value={percent} format={(v) => number(Math.round(v) / 100, { style: "percent" })} />
          </span>
        </motion.span>
      )}
    </AnimatePresence>
  );
}

/** Who voted for what, an answer at a time: only for polls whose votes are public. */
function VotersDialog({
  open,
  onOpenChange,
  place,
  messageId,
  poll,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  place: PollPlaceValue;
  messageId: string;
  poll: Poll;
}) {
  const [answerId, setAnswerId] = useState(poll.answers[0]?.id ?? 0);
  const { t, number } = useI18n();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader title={t("chattools.poll.whoVoted")} description={poll.question} />
        <div role="tablist" aria-label={t("chattools.poll.answers")} className="scroll-thin -mx-1 mb-3 flex gap-1.5 overflow-x-auto px-1 pb-1">
          {poll.answers.map((a) => {
            const active = a.id === answerId;
            return (
              <button
                key={a.id}
                type="button"
                role="tab"
                aria-selected={active}
                onClick={() => setAnswerId(a.id)}
                className={cn(
                  "relative flex shrink-0 items-center gap-1.5 rounded-full border px-3 py-1.5 text-xs font-bold transition-colors",
                  active ? "border-transparent text-primary-foreground" : "text-muted-foreground hover:text-foreground",
                )}
              >
                {active && <motion.span layoutId={`voters-${messageId}`} transition={SPRING} className="absolute inset-0 rounded-full bg-primary" />}
                {a.emoji && <EmojiGlyph value={a.emoji} emojis={place.emojis} className="relative size-4" />}
                <span className="relative max-w-[10rem] truncate">{a.text}</span>
                <span className="relative tabular-nums opacity-80">{number(Number(a.votes))}</span>
              </button>
            );
          })}
        </div>
        {open && <VoterList key={answerId} place={place} messageId={messageId} answerId={answerId} />}
      </DialogContent>
    </Dialog>
  );
}

function VoterList({ place, messageId, answerId }: { place: PollPlaceValue; messageId: string; answerId: number }) {
  const [users, setUsers] = useState<User[] | null>(null);
  const [more, setMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { t } = useI18n();

  useEffect(() => {
    let live = true;
    run(listPollVoters(place.instanceKey, place.serverId, place.channelId, messageId, answerId))
      .then((res) => {
        if (!live) return;
        setUsers(res.users);
        setMore(res.hasMore);
      })
      .catch((err: Error) => live && setError(err.message));
    return () => {
      live = false;
    };
  }, [place.instanceKey, place.serverId, messageId, answerId]);

  async function loadMore() {
    const last = users?.at(-1);
    if (!last) return;
    try {
      const res = await run(listPollVoters(place.instanceKey, place.serverId, place.channelId, messageId, answerId, last.id));
      setUsers((list) => [...(list ?? []), ...res.users]);
      setMore(res.hasMore);
    } catch (err) {
      setError((err as Error).message);
    }
  }

  if (error) return <p className="text-sm text-destructive">{error}</p>;
  if (!users) return <LoaderCircleIcon className="mx-auto my-6 size-5 animate-spin text-muted-foreground" />;
  if (!users.length) return <p className="py-6 text-center text-sm text-muted-foreground">{t("chattools.poll.nobodyPicked")}</p>;
  return (
    <ul className="scroll-thin flex max-h-80 flex-col gap-1 overflow-y-auto">
      {users.map((user, n) => (
        <motion.li
          key={user.id}
          initial={{ opacity: 0, y: 6 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ ...SPRING, delay: Math.min(n, 12) * 0.025 }}
          className="flex items-center gap-2.5 rounded-xl px-2 py-1.5"
        >
          <UserAvatar user={user} className="size-8" />
          <span className="min-w-0 flex-1 truncate text-sm font-bold">{displayName(user)}</span>
          <span className="truncate text-xs text-muted-foreground">@{user.username}</span>
        </motion.li>
      ))}
      {more && (
        <li>
          <button type="button" onClick={loadMore} className="w-full rounded-xl py-2 text-xs font-bold text-primary hover:bg-primary/10">
            {t("chattools.poll.showMore")}
          </button>
        </li>
      )}
    </ul>
  );
}
