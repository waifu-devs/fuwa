import {
  ArrowDownIcon,
  BadgeCheckIcon,
  CheckIcon,
  ChevronLeftIcon,
  CopyIcon,
  KeyRoundIcon,
  RotateCcwKeyIcon,
  LockKeyholeIcon,
  PencilIcon,
  RotateCwIcon,
  SendHorizontalIcon,
  ShieldAlertIcon,
  ShieldOffIcon,
  Trash2Icon,
  UserRoundXIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { Conversation } from "@/gen/fuwa/v1/dm_pb";
import type { Member, User } from "@/gen/fuwa/v1/types_pb";
import type { Item } from "@/e2ee/vault";
import { MAX_DM } from "@/e2ee/engine";
import { focusChannel } from "@/fuwa/actions";
import { deleteDm, dismissDm, dmProblem, editDm, markDmRead, prepareConversation, retryDm, sendDm } from "@/fuwa/dms";
import { useFuwa, type PendingMessage } from "@/fuwa/store";
import { sendsMessage } from "@/components/chat/Composer";
import { DayDivider, EditBox, MessageBody, MessageLine, ToolButton } from "@/components/chat/MessageList";
import { EncryptionDialog } from "@/components/dm/EncryptionDialog";
import { CallButton, DmCallStrip } from "@/components/calls/DmCall";
import { UserAvatar } from "@/components/Icons";
import { SPRING, SwapText } from "@/components/motion";
import { useLayout } from "@/components/Shell";
import { displayName, formatFull, sameDay } from "@/lib/format";
import { comboLabel } from "@/lib/keybinds";
import { setTitle } from "@/lib/notify";
import { usePrefs, type MessageDisplay } from "@/lib/prefs";
import { cn } from "@/lib/utils";

/** Messages from one person closer together than this share a header. */
const GROUP_GAP_MS = 7 * 60 * 1000;
const NO_ITEMS: Item[] = [];
const NO_PENDING: PendingMessage[] = [];
const drafts = new Map<string, string>();

/** An encrypted conversation: its people, what was said, and where you write. */
export function DmView({ instanceKey, conversationId }: { instanceKey: string; conversationId: string }) {
  const status = useFuwa((s) => s.instances[instanceKey]?.dms.status ?? "off");
  const problem = useFuwa((s) => s.instances[instanceKey]?.dms.problem ?? null);
  const conversation = useFuwa((s) => s.instances[instanceKey]?.dms.conversations.find((c) => c.id === conversationId));
  const me = useFuwa((s) => s.instances[instanceKey]?.me ?? undefined);
  const { compact, setNavOpen } = useLayout();
  const [sheet, setSheet] = useState(false);
  const partner = conversation?.users.find((u) => u.id !== me?.id) ?? conversation?.users[0];

  useEffect(() => {
    focusChannel(instanceKey, conversationId);
    void markDmRead(instanceKey, conversationId);
    return () => focusChannel(null, null);
  }, [instanceKey, conversationId]);

  useEffect(() => {
    setTitle(partner ? `${displayName(partner)} · fuwa` : "fuwa");
  }, [partner]);
  useEffect(() => () => setTitle("fuwa"), []);

  // Once encryption is running here: catch up, and make sure everyone's devices are in.
  const known = !!conversation;
  useEffect(() => {
    if (status === "ready" && known) void prepareConversation(instanceKey, conversationId).catch(() => {});
  }, [status, known, instanceKey, conversationId]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-14 shrink-0 items-center gap-2 border-b px-2 sm:px-4">
        {compact && (
          <button
            type="button"
            aria-label="Conversations"
            onClick={() => setNavOpen(true)}
            className="grid size-9 place-items-center rounded-full text-muted-foreground transition hover:-translate-x-0.5 hover:bg-muted"
          >
            <ChevronLeftIcon className="size-5" />
          </button>
        )}
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span
            key={partner?.id ?? "none"}
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -10 }}
            transition={SPRING}
            className="flex min-w-0 shrink items-center gap-2.5"
          >
            <UserAvatar user={partner} className="size-8 text-xs" />
            <span className="min-w-0">
              <h1 className="truncate leading-tight font-extrabold">
                <SwapText className="truncate align-bottom">{partner ? displayName(partner) : "Conversation"}</SwapText>
              </h1>
              {partner && <p className="truncate text-xs leading-tight text-muted-foreground">@{partner.username}</p>}
            </span>
          </motion.span>
        </AnimatePresence>
        <span className="flex-1" />
        {conversation && status === "ready" && <CallButton instanceKey={instanceKey} conversationId={conversationId} />}
        {conversation && <TrustPill instanceKey={instanceKey} conversationId={conversationId} onOpen={() => setSheet(true)} />}
      </header>
      {conversation && me ? (
        <>
          <DmCallStrip instanceKey={instanceKey} conversation={conversation} me={me} />
          <DmMessages instanceKey={instanceKey} conversation={conversation} me={me} partner={partner} />
          <EncryptedComposer
            instanceKey={instanceKey}
            id={conversation.id}
            placeholder={`Message @${partner?.username ?? "them"}`}
            promise="Only you two can read this"
          />
          <EncryptionDialog open={sheet} onOpenChange={setSheet} instanceKey={instanceKey} conversation={conversation} />
        </>
      ) : status === "unsupported" || status === "failed" ? (
        <Unavailable text={problem ?? "Encrypted messages aren't available here."} />
      ) : status === "ready" ? (
        <Unavailable text="This conversation isn't here, or you're not in it." icon={UserRoundXIcon} />
      ) : (
        <Starting />
      )}
    </div>
  );
}

/** The lock in the header: encrypted always; verified once you've checked the safety number; a warning if it changed. */
function TrustPill({ instanceKey, conversationId, onOpen }: { instanceKey: string; conversationId: string; onOpen: () => void }) {
  const safety = useFuwa((s) => s.instances[instanceKey]?.dms.safety[conversationId] ?? "");
  const verified = useFuwa((s) => s.instances[instanceKey]?.dms.verified[conversationId] ?? "");
  const state = safety && verified === safety ? "verified" : verified && safety ? "changed" : "encrypted";
  const Icon = state === "verified" ? BadgeCheckIcon : state === "changed" ? ShieldAlertIcon : LockKeyholeIcon;
  const label = state === "verified" ? "Verified" : state === "changed" ? "Safety number changed" : "End-to-end encrypted";
  return (
    <motion.button
      type="button"
      onClick={onOpen}
      whileTap={{ scale: 0.92 }}
      initial={{ opacity: 0, scale: 0.8 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={SPRING}
      title="See how this conversation is encrypted"
      className={cn(
        "group relative flex shrink-0 items-center gap-1.5 overflow-hidden rounded-full px-2.5 py-1 text-xs font-bold transition-colors",
        state === "changed"
          ? "bg-amber-500/15 text-amber-700 hover:bg-amber-500/25 dark:text-amber-300"
          : "bg-emerald-500/12 text-emerald-700 hover:bg-emerald-500/20 dark:text-emerald-300",
      )}
    >
      <span aria-hidden className="shine pointer-events-none absolute inset-0" />
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={state}
          initial={{ scale: 0.3, rotate: -40, opacity: 0 }}
          animate={{ scale: 1, rotate: 0, opacity: 1 }}
          exit={{ scale: 0.3, opacity: 0 }}
          transition={{ type: "spring", stiffness: 600, damping: 16 }}
          className="grid place-items-center"
        >
          <Icon className="size-3.5 transition-transform duration-300 group-hover:-rotate-12 group-hover:scale-110" />
        </motion.span>
      </AnimatePresence>
      <span className="hidden sm:inline">{label}</span>
    </motion.button>
  );
}

export function Starting() {
  return (
    <div className="grid flex-1 place-items-center p-6 text-center">
      <div className="flex flex-col items-center">
        <motion.span
          animate={{ rotate: [0, -18, 14, -8, 0] }}
          transition={{ duration: 1.6, repeat: Infinity, repeatDelay: 0.4, ease: "easeInOut" }}
          className="grid size-14 place-items-center rounded-2xl bg-emerald-500/15 text-emerald-600 dark:text-emerald-400"
        >
          <KeyRoundIcon className="size-7" />
        </motion.span>
        <p className="mt-4 font-extrabold">Setting up encryption on this device</p>
        <p className="mt-1 max-w-xs text-sm text-muted-foreground">Making this device's keys. It only takes a moment, once.</p>
      </div>
    </div>
  );
}

export function Unavailable({ text, icon: Icon = ShieldOffIcon }: { text: string; icon?: typeof ShieldOffIcon }) {
  return (
    <div className="grid flex-1 place-items-center p-6 text-center">
      <motion.div initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col items-center">
        <span className="grid size-14 place-items-center rounded-2xl bg-muted text-muted-foreground">
          <Icon className="size-7" />
        </span>
        <p className="mt-4 max-w-sm text-sm text-muted-foreground">{text}</p>
      </motion.div>
    </div>
  );
}

type Row =
  | { kind: "day"; key: string; date: Date }
  | { kind: "item"; key: string; item: Item; first: boolean; date: Date }
  | { kind: "pending"; key: string; pending: PendingMessage; first: boolean };

function DmMessages({
  instanceKey,
  conversation,
  me,
  partner,
}: {
  instanceKey: string;
  conversation: Conversation;
  me: User;
  partner: User | undefined;
}) {
  const users = useMemo(() => new Map(conversation.users.map((u) => [u.id, u])), [conversation.users]);
  const userOf = useCallback((id: string) => users.get(id), [users]);
  const describe = useCallback((item: Item) => deviceLine(item, users, me), [users, me]);
  return (
    <EncryptedMessages
      instanceKey={instanceKey}
      id={conversation.id}
      me={me}
      userOf={userOf}
      describe={describe}
      beginning={<Beginning partner={partner} />}
      deleteQuestion="Delete for both of you?"
      joiningText="Unlocking the conversation on this device…"
    />
  );
}

/**
 * What an encrypted conversation or secure channel said, as this device
 * opened it: messages, device lines, and what you're sending. The same list
 * for direct messages and secure channels.
 */
export function EncryptedMessages({
  instanceKey,
  id,
  me,
  userOf,
  memberOf,
  describe,
  beginning,
  canModerate = false,
  deleteQuestion,
  joiningText,
}: {
  instanceKey: string;
  id: string;
  me: User;
  userOf: (id: string) => User | undefined;
  memberOf?: (id: string) => Member | undefined;
  /** A device line, in words. */
  describe: (item: Item) => string;
  beginning: ReactNode;
  /** Can delete other people's messages too. */
  canModerate?: boolean;
  deleteQuestion: string;
  joiningText: string;
}) {
  const items = useFuwa((s) => s.instances[instanceKey]?.dms.items[id]);
  const pending = useFuwa((s) => s.instances[instanceKey]?.dms.pending[id] ?? NO_PENDING);
  const joining = useFuwa((s) => !!s.instances[instanceKey]?.dms.joining[id]);
  const display = usePrefs((p) => p.messageDisplay);
  usePrefs((p) => p.clock);
  const [editing, setEditing] = useState<number | null>(null);
  const list = items ?? NO_ITEMS;

  const rows = useMemo(() => {
    const out: Row[] = [];
    let prev: { author: string; at: Date } | null = null;
    for (const item of list) {
      // The first device to join a conversation doesn't need telling it joined.
      if (item.kind === "joined" && item.seq <= 1) continue;
      const date = dateOf(item);
      if (!prev || !sameDay(prev.at, date)) {
        out.push({ kind: "day", key: `day-${date.toDateString()}`, date });
        prev = null;
      }
      if (item.kind !== "text") {
        out.push({ kind: "item", key: `s${item.seq}`, item, first: true, date });
        prev = { author: "", at: date };
        continue;
      }
      const first = !prev || prev.author !== item.senderId || date.getTime() - prev.at.getTime() > GROUP_GAP_MS;
      out.push({ kind: "item", key: `s${item.seq}`, item, first, date });
      prev = { author: item.senderId, at: date };
    }
    for (const p of pending) {
      const first = !prev || prev.author !== me.id || p.createdAt - prev.at.getTime() > GROUP_GAP_MS;
      out.push({ kind: "pending", key: p.nonce, pending: p, first });
      prev = { author: me.id, at: new Date(p.createdAt) };
    }
    return out;
  }, [list, pending, me.id]);

  // Only what arrives after opening animates in.
  const initial = useRef<Set<number> | null>(null);
  if (initial.current === null && items) initial.current = new Set(items.map((i) => i.seq));

  // A long conversation opens with its latest rows drawn; scrolling up reveals the rest.
  const [hidden, setHidden] = useState<number | null>(null);
  const skipped = hidden ?? (items ? Math.max(0, rows.length - FIRST_ROWS) : 0);
  if (hidden === null && items) setHidden(skipped);
  const shown = skipped ? rows.slice(skipped) : rows;

  const actions = useMemo<DmActions>(
    () => ({
      edit: setEditing,
      cancelEdit: () => setEditing(null),
      save: async (seq, text) => {
        await editDm(instanceKey, id, seq, text);
        setEditing(null);
      },
      remove: (seq) => deleteDm(instanceKey, id, seq),
    }),
    [instanceKey, id],
  );

  const scroller = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const fromBottom = useRef(0);
  const [missed, setMissed] = useState(0);
  const count = useRef(0);
  const shownBefore = useRef(skipped);
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const revealed = skipped < shownBefore.current;
    if (atBottom.current) el.scrollTop = el.scrollHeight;
    // Rows revealed above: keep what you were reading where it was.
    else if (revealed) el.scrollTop = el.scrollHeight - fromBottom.current;
    else if (rows.length > count.current) setMissed((n) => n + rows.length - count.current);
    count.current = rows.length;
    shownBefore.current = skipped;
  }, [rows, skipped]);
  const onScroll = useCallback(() => {
    const el = scroller.current;
    if (!el) return;
    fromBottom.current = el.scrollHeight - el.scrollTop;
    atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
    if (atBottom.current) setMissed(0);
    if (el.scrollTop < 300 && skipped > 0) setHidden(Math.max(0, skipped - MORE_ROWS));
  }, [skipped]);

  return (
    <div className="relative min-h-0 flex-1">
      <div ref={scroller} onScroll={onScroll} className="scroll-thin h-full overflow-y-auto [overflow-anchor:none]">
        <motion.div
          initial={{ opacity: 0, y: 12 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.35, ease: [0.22, 1, 0.36, 1] }}
          className="flex min-h-full flex-col justify-end pb-3"
        >
          {skipped === 0 && beginning}
          {(!items || joining) && <Joining text={joiningText} />}
          {/* Rows don't animate their layout, so a new arrival needn't re-render every row (the default does). */}
          <AnimatePresence initial={false} presenceAffectsLayout={false}>
            {shown.map((row) => {
              if (row.kind === "day") return <DayDivider key={row.key} date={row.date} />;
              if (row.kind === "pending")
                return (
                  <PendingDm
                    key={row.key}
                    pending={row.pending}
                    first={row.first}
                    me={me}
                    onRetry={() => void retryDm(instanceKey, id, row.pending)}
                    onDismiss={() => dismissDm(instanceKey, id, row.pending.nonce)}
                  />
                );
              const item = row.item;
              const animate = !initial.current?.has(item.seq);
              if (item.kind !== "text") return <SystemLine key={row.key} item={item} text={describe(item)} animate={animate} />;
              const mine = item.senderId === me.id;
              return (
                <DmRow
                  key={row.key}
                  item={item}
                  first={row.first}
                  date={row.date}
                  author={userOf(item.senderId)}
                  member={memberOf?.(item.senderId)}
                  display={display}
                  mine={mine}
                  deletable={mine || canModerate}
                  deleteQuestion={deleteQuestion}
                  instanceKey={instanceKey}
                  animate={animate}
                  editing={editing === item.seq}
                  actions={actions}
                />
              );
            })}
          </AnimatePresence>
        </motion.div>
      </div>
      <AnimatePresence>
        {missed > 0 && (
          <motion.button
            type="button"
            onClick={() => {
              const el = scroller.current;
              if (!el) return;
              atBottom.current = true;
              el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
              setMissed(0);
            }}
            initial={{ opacity: 0, y: 16, scale: 0.9 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 16, scale: 0.9 }}
            transition={{ type: "spring", stiffness: 500, damping: 30 }}
            className="absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-2 rounded-full bg-primary px-4 py-2 text-sm font-bold text-primary-foreground shadow-lg"
          >
            <ArrowDownIcon className="size-4 animate-bounce" />
            {missed} new {missed === 1 ? "message" : "messages"}
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}

function Beginning({ partner }: { partner: User | undefined }) {
  return (
    <motion.div initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} transition={{ ...SPRING, delay: 0.05 }} className="px-4 pt-10 pb-4">
      <span className="relative inline-flex">
        <UserAvatar user={partner} className="size-20 text-3xl ring-4 ring-card" />
        <motion.span
          initial={{ scale: 0, rotate: -30 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.45 }}
          className="absolute -right-1 -bottom-1 grid size-8 place-items-center rounded-full bg-emerald-500 text-white ring-4 ring-card"
        >
          <LockKeyholeIcon className="size-4" />
        </motion.span>
      </span>
      <h2 className="mt-3 text-2xl font-extrabold sm:text-3xl">{displayName(partner)}</h2>
      {partner && <p className="text-muted-foreground">@{partner.username}</p>}
      <p className="mt-3 flex max-w-xl items-start gap-2 rounded-2xl bg-emerald-500/10 px-3 py-2.5 text-sm text-emerald-900 dark:text-emerald-100">
        <LockKeyholeIcon className="mt-0.5 size-4 shrink-0 text-emerald-600 dark:text-emerald-400" />
        <span>
          This conversation is <b>end-to-end encrypted</b>. Only you and {displayName(partner)} can read it, on the devices you're signed in on.
          Not even this fuwa server can.
        </span>
      </p>
    </motion.div>
  );
}

function Joining({ text }: { text: string }) {
  return (
    <div className="flex items-center gap-3 px-4 py-3 text-sm text-muted-foreground">
      <motion.span
        animate={{ rotate: [0, -16, 12, 0] }}
        transition={{ duration: 1.2, repeat: Infinity, ease: "easeInOut" }}
        className="grid size-8 place-items-center rounded-full bg-emerald-500/15 text-emerald-600 dark:text-emerald-400"
      >
        <KeyRoundIcon className="size-4" />
      </motion.span>
      {text}
    </div>
  );
}

const enter = { initial: { opacity: 0, y: 12, scale: 0.98 }, animate: { opacity: 1, y: 0, scale: 1 } };

/** Rows drawn when a conversation opens; older ones come in as you scroll up. */
const FIRST_ROWS = 80;
const MORE_ROWS = 80;

/** One date per (immutable) item, so memoized rows keep the same props. */
const dates = new WeakMap<Item, Date>();
const dateOf = (item: Item) => {
  let d = dates.get(item);
  if (!d) dates.set(item, (d = new Date(item.at)));
  return d;
};

/** What a row can do to its message, the same object for the whole conversation. */
type DmActions = {
  edit: (seq: number) => void;
  cancelEdit: () => void;
  save: (seq: number, text: string) => Promise<void>;
  remove: (seq: number) => Promise<void>;
};

const DmRow = memo(function DmRow({
  item,
  first,
  date,
  author,
  member,
  display,
  mine,
  deletable,
  deleteQuestion,
  instanceKey,
  animate,
  editing,
  actions,
}: {
  item: Item;
  first: boolean;
  date: Date;
  author: User | undefined;
  member: Member | undefined;
  display: MessageDisplay;
  mine: boolean;
  deletable: boolean;
  deleteQuestion: string;
  instanceKey: string;
  animate: boolean;
  editing: boolean;
  actions: DmActions;
}) {
  const [confirming, setConfirming] = useState(false);
  const [copied, setCopied] = useState(false);
  return (
    <motion.div
      {...(animate ? enter : {})}
      exit={{ opacity: 0, height: 0, transition: { duration: 0.2 } }}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      className={cn("message-row group relative flex gap-3 px-4", first && "first", display === "compact" && "compact", animate && mine && "landed")}
    >
      <MessageLine display={display} first={first} author={author} member={member} date={date} instanceKey={instanceKey}>
        {item.deleted ? (
          <p className="text-sm text-muted-foreground italic">Message deleted</p>
        ) : editing ? (
          <EditBox initial={item.content} onCancel={actions.cancelEdit} onSave={(text) => actions.save(item.seq, text)} />
        ) : (
          <>
            <MessageBody content={item.content} display={display} />
            {item.editedAt > 0 && (
              <span className="text-[0.7rem] text-muted-foreground" title={formatFull(new Date(item.editedAt))}>
                {" "}
                (edited)
              </span>
            )}
          </>
        )}
      </MessageLine>
      {!editing && !item.deleted && (
        <div className="message-tools absolute -top-3 right-4 z-10 flex items-center gap-0.5 rounded-xl border bg-card p-0.5 shadow-md">
          {confirming ? (
            <motion.span
              initial={{ opacity: 0, x: 8 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ type: "spring", stiffness: 600, damping: 32 }}
              className="flex items-center gap-0.5"
            >
              <span className="px-2 text-xs font-bold text-destructive">{deleteQuestion}</span>
              <ToolButton label="Delete" danger onClick={() => actions.remove(item.seq).catch(() => setConfirming(false))}>
                <CheckIcon />
              </ToolButton>
              <ToolButton label="Keep" onClick={() => setConfirming(false)}>
                <XIcon />
              </ToolButton>
            </motion.span>
          ) : (
            <>
              <ToolButton
                label={copied ? "Copied" : "Copy text"}
                onClick={() => {
                  void navigator.clipboard?.writeText(item.content);
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1200);
                }}
              >
                <AnimatePresence mode="wait" initial={false}>
                  <motion.span
                    key={copied ? "copied" : "copy"}
                    initial={{ scale: 0.3, rotate: copied ? -45 : 0, opacity: 0 }}
                    animate={{ scale: 1, rotate: 0, opacity: 1 }}
                    exit={{ scale: 0.3, opacity: 0 }}
                    transition={{ type: "spring", stiffness: 700, damping: 22 }}
                    className="grid place-items-center"
                  >
                    {copied ? <CheckIcon className="text-primary" /> : <CopyIcon />}
                  </motion.span>
                </AnimatePresence>
              </ToolButton>
              {mine && (
                <ToolButton label="Edit" onClick={() => actions.edit(item.seq)}>
                  <PencilIcon />
                </ToolButton>
              )}
              {deletable && (
                <ToolButton label="Delete" danger onClick={() => setConfirming(true)}>
                  <Trash2Icon />
                </ToolButton>
              )}
            </>
          )}
        </div>
      )}
    </motion.div>
  );
});

/** What changed about the conversation's devices, in words. */
function deviceLine(item: Item, users: Map<string, User>, me: User): string {
  const name = (id: string) => (id === me.id ? "you" : displayName(users.get(id)));
  const whose = (id: string) => (id === me.id ? "your" : `${displayName(users.get(id))}'s`);
  const capital = (text: string) => `${text[0]?.toUpperCase() ?? ""}${text.slice(1)}`;
  if (item.kind === "joined") return "This device joined the conversation. Messages from before it can't be read here.";
  if (item.kind === "unreadable") return `A message from ${name(item.senderId)} couldn't be opened on this device.`;
  // The conversation's first record is the commit that made its group.
  if (item.seq === 1) return `${capital(name(item.senderId))} started this encrypted conversation.`;
  const people = new Set(item.added.map((d) => d.userId));
  const parts: string[] = [];
  for (const userId of people) {
    const n = item.added.filter((d) => d.userId === userId).length;
    parts.push(`${name(userId)} signed in on ${n === 1 ? "a new device" : `${n} new devices`}`);
  }
  for (const userId of new Set(item.removed.map((d) => d.userId))) {
    const n = item.removed.filter((d) => d.userId === userId).length;
    parts.push(n === 1 ? `one of ${whose(userId)} devices signed out` : `${n} of ${whose(userId)} devices signed out`);
  }
  return `${capital(parts.join(", and "))}. The safety number changed.`;
}

function SystemLine({ item, text, animate }: { item: Item; text: string; animate: boolean }) {
  const Icon = item.kind === "unreadable" ? ShieldAlertIcon : item.kind === "reset" ? RotateCcwKeyIcon : KeyRoundIcon;
  return (
    <motion.div
      {...(animate ? enter : {})}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      className="message-row group flex items-center gap-3 px-4 py-1.5"
    >
      <span className="grid w-10 shrink-0 place-items-center">
        <Icon
          className={cn(
            "size-4 transition-transform duration-500 group-hover:rotate-[-20deg]",
            item.kind === "unreadable" || item.kind === "reset" ? "text-amber-500" : "text-emerald-500",
          )}
        />
      </span>
      <p className="min-w-0 flex-1 text-[0.9rem] text-muted-foreground">
        {text}{" "}
        <time className="text-xs whitespace-nowrap" dateTime={new Date(item.at).toISOString()} title={formatFull(new Date(item.at))}>
          {new Date(item.at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}
        </time>
      </p>
    </motion.div>
  );
}

function PendingDm({
  pending,
  first,
  me,
  onRetry,
  onDismiss,
}: {
  pending: PendingMessage;
  first: boolean;
  me: User;
  onRetry: () => void;
  onDismiss: () => void;
}) {
  const display = usePrefs((p) => p.messageDisplay);
  return (
    <motion.div
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: pending.failed ? 1 : 0.55, y: 0 }}
      exit={{ opacity: 0 }}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      className={cn("message-row flex gap-3 px-4", first && "first", display === "compact" && "compact")}
    >
      <MessageLine display={display} first={first} author={me} member={undefined} status="encrypting…">
        <MessageBody content={pending.content} display={display} className={cn(pending.failed && "text-destructive")} />
        {pending.failed && (
          <p className="mt-1 flex flex-wrap items-center gap-2 text-xs">
            <span className="text-destructive first-letter:uppercase">{pending.failed.replace(/\.$/, "")}.</span>
            <button type="button" onClick={onRetry} className="inline-flex items-center gap-1 font-bold text-primary hover:underline">
              <RotateCwIcon className="size-3" /> Retry
            </button>
            <button type="button" onClick={onDismiss} className="font-bold text-muted-foreground hover:underline">
              Dismiss
            </button>
          </p>
        )}
      </MessageLine>
    </motion.div>
  );
}

/**
 * Where you write in an encrypted conversation or secure channel. Sending
 * seals the message (a lock clicks shut) and flies it off. `locked` says why
 * you can't write here at all (no permission to), if you can't.
 */
export function EncryptedComposer({
  instanceKey,
  id,
  placeholder,
  promise,
  locked = "",
  action,
}: {
  instanceKey: string;
  id: string;
  placeholder: string;
  promise: string;
  locked?: string;
  /** Shown where "Try again" is when you can't write; null for nothing. */
  action?: ReactNode;
}) {
  const status = useFuwa((s) => s.instances[instanceKey]?.dms.status ?? "off");
  const stuck = useFuwa((s) => s.instances[instanceKey]?.dms.blocked[id] ?? "");
  const blocked = locked || stuck;
  const [text, setText] = useState(() => drafts.get(id) ?? "");
  const box = useRef<HTMLTextAreaElement>(null);
  const plane = useAnimationControls();
  const seal = useAnimationControls();
  const sendWith = usePrefs((p) => p.sendWith);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setText(drafts.get(id) ?? "");
    if (window.matchMedia("(pointer: fine)").matches) box.current?.focus();
  }, [id]);
  useEffect(() => {
    drafts.set(id, text);
  }, [id, text]);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(el.scrollHeight, window.innerHeight * 0.4)}px`;
  }, [text]);

  const content = text.trim();
  const tooLong = text.length > MAX_DM;
  const ready = !!content && !tooLong && status === "ready";

  function send() {
    if (!ready) return;
    setText("");
    drafts.delete(id);
    setError(null);
    void seal.start({ rotate: [0, -16, 10, 0], scale: [1, 1.3, 1], transition: { duration: 0.45 } });
    void plane.start({
      x: [0, 28, -18, 0],
      y: [0, -14, 8, 0],
      opacity: [1, 0, 0, 1],
      rotate: [0, -20, 0, 0],
      transition: { duration: 0.55, times: [0, 0.45, 0.5, 1], ease: "easeOut" },
    });
    sendDm(instanceKey, id, content).catch((err: unknown) => setError(dmProblem(err)));
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.nativeEvent.isComposing) return;
    if (sendsMessage(e, sendWith)) {
      e.preventDefault();
      send();
    }
  }

  return (
    <div className="px-3 pb-[max(0.75rem,env(safe-area-inset-bottom))] sm:px-4">
      <AnimatePresence mode="popLayout" initial={false}>
        {blocked ? (
          <motion.div
            key="blocked"
            initial={{ opacity: 0, y: 12, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -8, scale: 0.98 }}
            transition={SPRING}
            role="status"
            className="flex items-center gap-3 rounded-2xl border border-dashed bg-muted/40 px-3 py-2.5"
          >
            <motion.span
              initial={{ rotate: -20, scale: 0.6 }}
              animate={{ rotate: [0, -10, 8, 0], scale: 1 }}
              transition={{ ...SPRING, rotate: { duration: 0.6, delay: 0.1 } }}
              className="grid size-9 shrink-0 place-items-center rounded-xl bg-muted text-muted-foreground"
            >
              <KeyRoundIcon className="size-[18px]" />
            </motion.span>
            <div className="min-w-0 flex-1">
              <p className="text-sm font-bold">{locked ? "You can read this, but not write here" : "You can't write here yet"}</p>
              <p className="text-xs text-muted-foreground">{blocked}</p>
            </div>
            {action !== undefined
              ? action
              : !locked && (
                  <button
                    type="button"
                    onClick={() => void prepareConversation(instanceKey, id).catch(() => {})}
                    className="shrink-0 rounded-xl px-3 py-1.5 text-xs font-bold text-primary transition hover:bg-primary/10"
                  >
                    Try again
                  </button>
                )}
          </motion.div>
        ) : (
          <motion.div
            key="composer"
            initial={{ opacity: 0, y: 12, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 12, scale: 0.98 }}
            transition={SPRING}
            className="composer relative flex items-end gap-2 rounded-2xl border bg-card px-3 py-2"
          >
            <motion.span
              animate={seal}
              title="Encrypted on this device before it's sent"
              className="mb-2 grid size-5 shrink-0 place-items-center text-emerald-500"
            >
              <LockKeyholeIcon className="size-4" />
            </motion.span>
            <textarea
              ref={box}
              data-composer
              rows={1}
              value={text}
              onChange={(e) => setText(e.target.value)}
              onKeyDown={onKeyDown}
              placeholder={placeholder}
              aria-label={placeholder}
              className="scroll-thin max-h-[40vh] min-h-6 flex-1 resize-none bg-transparent py-1.5 text-[0.95rem] leading-6 outline-none placeholder:text-muted-foreground"
            />
            <AnimatePresence>
              {text.length > MAX_DM - 500 && (
                <motion.span
                  initial={{ opacity: 0, scale: 0.8 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.8 }}
                  className={cn("mb-2 text-xs tabular-nums", tooLong ? "font-bold text-destructive" : "text-muted-foreground")}
                >
                  {MAX_DM - text.length}
                </motion.span>
              )}
            </AnimatePresence>
            <motion.button
              type="button"
              onClick={send}
              disabled={!ready}
              aria-label="Send"
              whileTap={{ scale: 0.85 }}
              initial={false}
              animate={{ scale: ready ? 1 : 0.9 }}
              transition={{ type: "spring", stiffness: 600, damping: 20 }}
              className={cn(
                "relative mb-0.5 grid size-9 shrink-0 place-items-center rounded-xl transition-colors",
                ready ? "bg-primary text-primary-foreground shadow-[0_6px_18px_-8px_var(--primary)]" : "text-muted-foreground",
              )}
            >
              <motion.span animate={plane} className="block">
                <SendHorizontalIcon className="size-[18px]" />
              </motion.span>
            </motion.button>
          </motion.div>
        )}
      </AnimatePresence>
      <div className={cn("mt-1 flex items-center gap-3 px-1 text-[0.7rem] text-muted-foreground transition-opacity", blocked && "invisible opacity-0")}>
        <p className="hidden min-w-0 flex-1 truncate sm:block">
          <b>{sendWith === "enter" ? comboLabel("Enter") : comboLabel("Mod+Enter")}</b> to send ·{" "}
          <b>{sendWith === "enter" ? comboLabel("Shift+Enter") : comboLabel("Enter")}</b> for a new line · Markdown works
        </p>
        <p className="ml-auto flex shrink-0 items-center gap-1 font-bold text-emerald-600 dark:text-emerald-400">
          <LockKeyholeIcon className="size-3" /> {error ?? promise}
        </p>
      </div>
    </div>
  );
}
