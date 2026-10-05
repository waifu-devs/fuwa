import {
  ArrowDownIcon,
  BadgeCheckIcon,
  CheckIcon,
  ChevronLeftIcon,
  CopyIcon,
  HistoryIcon,
  MessageSquareReplyIcon,
  KeyRoundIcon,
  LockIcon,
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
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { Conversation } from "@/gen/fuwa/v1/dm_pb";
import type { Member, User } from "@/gen/fuwa/v1/types_pb";
import { isMessage, type Item } from "@/e2ee/vault";
import { MAX_DM } from "@/e2ee/engine";
import { focusChannel } from "@/fuwa/actions";
import {
  deleteDm,
  dismissPending,
  dmProblem,
  editDm,
  markDmRead,
  prepareConversation,
  retryPending,
  sendDm,
  sendDmFiles,
  sendVoiceDm,
  voiceLimits,
  voiceLoader,
  type ThreadTarget,
} from "@/fuwa/dms";
import { useFuwa, type PendingMessage } from "@/fuwa/store";
import { sendsMessage } from "@/components/chat/Composer";
import { TimestampPicker } from "@/components/chat/TimestampPicker";
import { insertAtCaret } from "@/lib/caret";
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
import { type I18n, T, useI18n } from "@/i18n/react";
import { VoiceMessage, VoiceProblem } from "@/components/voice/VoiceMessage";
import { VoiceRecorder } from "@/components/voice/VoiceRecorder";
import { PendingFiles, SealedFiles } from "@/components/dm/SealedFiles";
import { clearPicked, EncryptedAttach, PickedTray, pickFiles, usePicked } from "@/components/dm/EncryptedFiles";
import { DropOverlay } from "@/components/chat/ComposerFiles";

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
  const { t } = useI18n();
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
            aria-label={t("dms-calls.dm.view.back")}
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
                <SwapText className="truncate align-bottom">{partner ? displayName(partner) : t("dms-calls.dm.view.untitled")}</SwapText>
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
            placeholder={t("dms-calls.dm.view.placeholder", { name: partner?.username ?? t("dms-calls.dm.view.them") })}
            promise={t("dms-calls.dm.view.promise")}
            voice
            files
            dropTo={`@${partner?.username ?? t("dms-calls.dm.view.them")}`}
          />
          <EncryptionDialog open={sheet} onOpenChange={setSheet} instanceKey={instanceKey} conversation={conversation} />
        </>
      ) : status === "unsupported" || status === "failed" ? (
        <Unavailable text={problem ?? t("dms-calls.dm.unavailable")} />
      ) : status === "ready" ? (
        <Unavailable text={t("dms-calls.dm.view.notHere")} icon={UserRoundXIcon} />
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
  const { t } = useI18n();
  const state = safety && verified === safety ? "verified" : verified && safety ? "changed" : "encrypted";
  const Icon = state === "verified" ? BadgeCheckIcon : state === "changed" ? ShieldAlertIcon : LockKeyholeIcon;
  const label = t(state === "verified" ? "dms-calls.dm.trust.verified" : state === "changed" ? "dms-calls.dm.trust.changed" : "dms-calls.dm.encrypted");
  return (
    <motion.button
      type="button"
      onClick={onOpen}
      whileTap={{ scale: 0.92 }}
      initial={{ opacity: 0, scale: 0.8 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={SPRING}
      title={t("dms-calls.dm.trust.title")}
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
  const { t } = useI18n();
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
        <p className="mt-4 font-extrabold">{t("dms-calls.dm.starting.title")}</p>
        <p className="mt-1 max-w-xs text-sm text-muted-foreground">{t("dms-calls.dm.starting.text")}</p>
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
  const earlier = useFuwa((s) => {
    const dms = s.instances[instanceKey]?.dms;
    return earlierFrom(dms?.items[conversation.id], dms?.backup.status === "locked");
  });
  const { t } = useI18n();
  const describe = useCallback((item: Item) => deviceLine(t, item, users, me, earlier), [t, users, me, earlier]);
  return (
    <EncryptedMessages
      instanceKey={instanceKey}
      id={conversation.id}
      me={me}
      userOf={userOf}
      describe={describe}
      beginning={<Beginning partner={partner} />}
      deleteQuestion={t("dms-calls.dm.view.deleteQuestion")}
      joiningText={t("dms-calls.dm.view.joining")}
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
  lines,
  pendingIn,
  threads,
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
  /** The lines to show, when not all of them (a secure channel without its threads' replies, or one thread). */
  lines?: Item[];
  /** Which messages being sent belong in this list. */
  pendingIn?: (p: PendingMessage) => boolean;
  /** A secure channel's threads: what shows under a line, and starting a thread on one. */
  threads?: ThreadHooks;
}) {
  const stored = useFuwa((s) => s.instances[instanceKey]?.dms.items[id]);
  const items = stored && (lines ?? stored);
  const allPending = useFuwa((s) => s.instances[instanceKey]?.dms.pending[id] ?? NO_PENDING);
  const pending = useMemo(() => (pendingIn ? allPending.filter(pendingIn) : allPending), [allPending, pendingIn]);
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
      if (!isMessage(item)) {
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
                    onRetry={() => void retryPending(instanceKey, id, row.pending)}
                    onDismiss={() => dismissPending(instanceKey, id, row.pending.nonce)}
                  />
                );
              const item = row.item;
              const animate = !initial.current?.has(item.seq);
              if (!isMessage(item)) return <SystemLine key={row.key} item={item} text={describe(item)} animate={animate} />;
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
                  deletable={canModerate || (mine && !threads?.kept(item))}
                  deleteQuestion={deleteQuestion}
                  instanceKey={instanceKey}
                  animate={animate}
                  editing={editing === item.seq}
                  actions={actions}
                  threads={threads}
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
            <T k="dms-calls.dm.messages.missed" values={{ count: missed }} />
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}

function Beginning({ partner }: { partner: User | undefined }) {
  const { t } = useI18n();
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
          <T k="dms-calls.dm.beginning.text" values={{ encrypted: <b>{t("dms-calls.dm.beginning.encrypted")}</b>, name: displayName(partner) }} />
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

/**
 * What a secure channel's list asks about threads, the same object while
 * its threads don't change: the row under a line (its replies, or that it
 * came from a thread), and whether a thread can start on it.
 */
export type ThreadHooks = {
  under: (item: Item) => ReactNode;
  canStart: (item: Item) => boolean;
  /** Opens the line's thread, starting it if it has none yet. */
  start: (item: Item) => void;
  has: (item: Item) => boolean;
  /** Whether its author can't delete it any more: someone else replied in its thread (moderators still can). */
  kept: (item: Item) => boolean;
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
  threads,
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
  threads?: ThreadHooks;
}) {
  const [confirming, setConfirming] = useState(false);
  const [copied, setCopied] = useState(false);
  const { t } = useI18n();
  return (
    <motion.div
      {...(animate ? enter : {})}
      exit={{ opacity: 0, height: 0, transition: { duration: 0.2 } }}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      className={cn("message-row group relative flex gap-3 px-4", first && "first", display === "compact" && "compact", animate && mine && "landed")}
    >
      <MessageLine display={display} first={first} author={author} member={member} date={date} instanceKey={instanceKey}>
        {item.deleted ? (
          <p className="text-sm text-muted-foreground italic">{t("dms-calls.dm.deleted")}</p>
        ) : item.kind === "voice" && item.voice ? (
          <DmVoice instanceKey={instanceKey} item={item} />
        ) : editing ? (
          <EditBox initial={item.content} onCancel={actions.cancelEdit} onSave={(text) => actions.save(item.seq, text)} />
        ) : (
          <>
            {(item.content || !item.files) && <MessageBody content={item.content} display={display} />}
            {item.editedAt > 0 && (
              <span className="text-[0.7rem] text-muted-foreground" title={formatFull(new Date(item.editedAt))}>
                {" "}
                {t("dms-calls.dm.row.edited")}
              </span>
            )}
            {item.sharedBy && (
              <span
                className="ml-1.5 inline-flex translate-y-[-1px] items-center gap-1 rounded-full bg-muted px-1.5 py-px align-middle text-[0.65rem] font-bold text-muted-foreground"
                title={t("dms-calls.dm.row.sharedTitle")}
              >
                <HistoryIcon className="size-3" />
                {t("dms-calls.dm.row.shared")}
              </span>
            )}
            {item.files && <SealedFiles instanceKey={instanceKey} files={item.files} animate={animate} />}
          </>
        )}
        {!editing && threads?.under(item)}
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
              <ToolButton label={t("dms-calls.dm.row.delete")} danger onClick={() => actions.remove(item.seq).catch(() => setConfirming(false))}>
                <CheckIcon />
              </ToolButton>
              <ToolButton label={t("dms-calls.dm.row.keep")} onClick={() => setConfirming(false)}>
                <XIcon />
              </ToolButton>
            </motion.span>
          ) : (
            <>
              {item.kind === "text" && !!item.content && (
                <ToolButton
                  label={copied ? t("dms-calls.dm.row.copied") : t("dms-calls.dm.row.copy")}
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
              )}
              {threads?.canStart(item) && (
                <ToolButton label={threads.has(item) ? t("dms-calls.dm.row.openThread") : t("dms-calls.dm.row.replyInThread")} onClick={() => threads.start(item)}>
                  <MessageSquareReplyIcon />
                </ToolButton>
              )}
              {mine && item.kind === "text" && (
                <ToolButton label={t("dms-calls.dm.row.edit")} onClick={() => actions.edit(item.seq)}>
                  <PencilIcon />
                </ToolButton>
              )}
              {deletable && (
                <ToolButton label={t("dms-calls.dm.row.delete")} danger onClick={() => setConfirming(true)}>
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

/** A voice message in a conversation: played from this instance, opened on this device. */
function DmVoice({ instanceKey, item }: { instanceKey: string; item: Item }) {
  const voice = item.voice!;
  const load = useMemo(() => voiceLoader(instanceKey, voice), [instanceKey, voice]);
  const id = `${instanceKey}:${item.conversation}:${item.seq}`;
  return (
    <>
      <VoiceMessage id={id} durationMs={voice.durationMs} waveform={voice.waveform} load={load} />
      <VoiceProblem id={id} />
    </>
  );
}

/** Where the messages from before this device joined came from: passed on by a member, or this account's backup. */
/** Or, with none here, "restorable" if the account's backup could bring them. */
export type Earlier = "shared" | "backup" | "restorable" | null;

export function earlierFrom(items: Item[] | undefined, locked: boolean): Earlier {
  if (items?.some((i) => i.sharedBy)) return "shared";
  const joined = items?.findLast((i) => i.kind === "joined");
  if (joined && items!.some((i) => isMessage(i) && i.seq < joined.seq)) return "backup";
  return locked ? "restorable" : null;
}

/** What changed about the conversation's devices, in words. */
function deviceLine(t: I18n["t"], item: Item, users: Map<string, User>, me: User, earlier: Earlier): string {
  const name = (id: string) => displayName(users.get(id));
  const capital = (text: string) => `${text[0]?.toUpperCase() ?? ""}${text.slice(1)}`;
  if (item.kind === "joined") {
    if (earlier === "backup") return t("dms-calls.dm.devices.joinedBackup");
    if (earlier === "restorable") return t("dms-calls.dm.devices.joinedRestorable");
    return t("dms-calls.dm.devices.joined");
  }
  if (item.kind === "unreadable")
    return item.senderId === me.id ? t("dms-calls.dm.devices.unreadableMine") : t("dms-calls.dm.devices.unreadable", { name: name(item.senderId) });
  // The conversation's first record is the commit that made its group.
  if (item.seq === 1)
    return capital(item.senderId === me.id ? t("dms-calls.dm.devices.startedMine") : t("dms-calls.dm.devices.started", { name: name(item.senderId) }));
  const people = new Set(item.added.map((d) => d.userId));
  const parts: string[] = [];
  for (const userId of people) {
    const count = item.added.filter((d) => d.userId === userId).length;
    parts.push(userId === me.id ? t("dms-calls.dm.devices.addedMine", { count }) : t("dms-calls.dm.devices.added", { count, name: name(userId) }));
  }
  for (const userId of new Set(item.removed.map((d) => d.userId))) {
    const count = item.removed.filter((d) => d.userId === userId).length;
    parts.push(userId === me.id ? t("dms-calls.dm.devices.removedMine", { count }) : t("dms-calls.dm.devices.removed", { count, name: name(userId) }));
  }
  const changes = parts.length ? parts.reduce((list, next) => t("dms-calls.dm.devices.and", { list, next })) : "";
  return capital(t("dms-calls.dm.devices.changed", { changes }));
}

function SystemLine({ item, text, animate }: { item: Item; text: string; animate: boolean }) {
  const { date } = useI18n();
  const Icon =
    item.kind === "unreadable"
      ? ShieldAlertIcon
      : item.kind === "reset"
        ? RotateCcwKeyIcon
        : item.kind === "setting"
          ? HistoryIcon
          : item.kind === "thread"
            ? LockIcon
            : KeyRoundIcon;
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
          {date(new Date(item.at), { hour: "numeric", minute: "2-digit" })}
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
  const { t } = useI18n();
  return (
    <motion.div
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: pending.failed ? 1 : 0.55, y: 0 }}
      exit={{ opacity: 0 }}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      className={cn("message-row flex gap-3 px-4", first && "first", display === "compact" && "compact")}
    >
      <MessageLine display={display} first={first} author={me} member={undefined} status={t("dms-calls.dm.pending.encrypting")}>
        {pending.voice ? (
          <VoiceMessage id={`pending:${pending.nonce}`} durationMs={pending.voice.durationMs} waveform={pending.voice.waveform} load={null} pending />
        ) : (
          pending.content && <MessageBody content={pending.content} display={display} className={cn(pending.failed && "text-destructive")} />
        )}
        {pending.sealed && <PendingFiles files={pending.sealed} />}
        {pending.failed && (
          <p className="mt-1 flex flex-wrap items-center gap-2 text-xs">
            <span className="text-destructive first-letter:uppercase">{pending.failed.replace(/\.$/, "")}.</span>
            <button type="button" onClick={onRetry} className="inline-flex items-center gap-1 font-bold text-primary hover:underline">
              <RotateCwIcon className="size-3" /> {t("dms-calls.dm.pending.retry")}
            </button>
            <button type="button" onClick={onDismiss} className="font-bold text-muted-foreground hover:underline">
              {t("dms-calls.dm.pending.dismiss")}
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
  thread,
  voice = false,
  files = false,
  dropTo = "",
}: {
  instanceKey: string;
  id: string;
  placeholder: string;
  promise: string;
  locked?: string;
  /** Offers voice messages (direct messages): the mic takes the send button's place while there's no text. */
  voice?: boolean;
  /** Offers files, sealed on this device (where you may attach them). */
  files?: boolean;
  /** Where dropped files go, as shown ("@mika", or a channel's name); "" takes no drops (a thread panel). */
  dropTo?: string;
  /** Shown where "Try again" is when you can't write; null for nothing. */
  action?: ReactNode;
  /** In a secure channel's thread: the thread's message, and the channel's name for "Also send to #channel". */
  thread?: { parent: number; channelName: string };
}) {
  const status = useFuwa((s) => s.instances[instanceKey]?.dms.status ?? "off");
  const stuck = useFuwa((s) => s.instances[instanceKey]?.dms.blocked[id] ?? "");
  const blocked = locked || stuck;
  const draft = thread ? `${id}#${thread.parent}` : id;
  const [text, setText] = useState(() => drafts.get(draft) ?? "");
  const [alsoChannel, setAlsoChannel] = useState(false);
  const box = useRef<HTMLTextAreaElement>(null);
  const plane = useAnimationControls();
  const seal = useAnimationControls();
  const sendWith = usePrefs((p) => p.sendWith);
  const [error, setError] = useState<string | null>(null);
  const { t } = useI18n();

  useEffect(() => {
    setText(drafts.get(draft) ?? "");
    if (window.matchMedia("(pointer: fine)").matches) box.current?.focus();
  }, [draft]);
  useEffect(() => {
    drafts.set(draft, text);
  }, [draft, text]);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(el.scrollHeight, window.innerHeight * 0.4)}px`;
  }, [text]);

  const picked = usePicked(draft);
  const content = text.trim();
  const tooLong = text.length > MAX_DM;
  const ready = (!!content || picked.length > 0) && !tooLong && status === "ready";
  const takeFiles = useCallback((list: File[]) => pickFiles(draft, list), [draft]);

  function send() {
    if (!ready) return;
    setText("");
    drafts.delete(draft);
    setError(null);
    void seal.start({ rotate: [0, -16, 10, 0], scale: [1, 1.3, 1], transition: { duration: 0.45 } });
    void plane.start({
      x: [0, 28, -18, 0],
      y: [0, -14, 8, 0],
      opacity: [1, 0, 0, 1],
      rotate: [0, -20, 0, 0],
      transition: { duration: 0.55, times: [0, 0.45, 0.5, 1], ease: "easeOut" },
    });
    const target: ThreadTarget | undefined = thread && { thread: thread.parent, inChannel: alsoChannel };
    setAlsoChannel(false);
    if (picked.length) {
      clearPicked(draft);
      void sendDmFiles(instanceKey, id, picked, content, target);
    } else {
      sendDm(instanceKey, id, content, target).catch((err: unknown) => setError(dmProblem(err)));
    }
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
      {files && dropTo && !blocked && status === "ready" && (
        <DropOverlay channelName={dropTo} onFiles={takeFiles} note={t("dms-calls.dm.composer.drop")} />
      )}
      <AnimatePresence initial={false}>{files && !blocked && picked.length > 0 && <PickedTray key="picked" draft={draft} files={picked} />}</AnimatePresence>
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
              <p className="text-sm font-bold">{locked ? t("dms-calls.dm.composer.locked") : t("dms-calls.dm.composer.blocked")}</p>
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
                    {t("dms-calls.dm.composer.tryAgain")}
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
              title={t("dms-calls.dm.composer.sealed")}
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
            {files && <EncryptedAttach draft={draft} disabled={status !== "ready"} />}
            <TimestampPicker onPick={(token) => insertAtCaret(box, setText, token)} />
            {voice && !content && !picked.length ? (
              <VoiceRecorder
                maxMs={() => voiceLimits(instanceKey).then((l) => l.maxMs)}
                onSend={(clip) => void sendVoiceDm(instanceKey, id, clip)}
                onProblem={setError}
                disabled={status !== "ready"}
              />
            ) : (
              <motion.button
                type="button"
                onClick={send}
                disabled={!ready}
                aria-label={t("dms-calls.dm.composer.send")}
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
            )}
          </motion.div>
        )}
      </AnimatePresence>
      <div className={cn("mt-1 flex items-center gap-3 px-1 text-[0.7rem] text-muted-foreground transition-opacity", blocked && "invisible opacity-0")}>
        {thread && (
          <label className="flex min-w-0 shrink cursor-pointer items-center gap-1.5 font-bold">
            <input
              type="checkbox"
              checked={alsoChannel}
              onChange={(e) => setAlsoChannel(e.target.checked)}
              className="size-3.5 accent-[var(--primary)]"
            />
            <span className="truncate">{t("dms-calls.dm.composer.alsoSend", { channel: thread.channelName })}</span>
          </label>
        )}
        <p className={cn("hidden min-w-0 flex-1 truncate", !thread && "sm:block")}>
          <T
            k="dms-calls.dm.composer.keys"
            values={{
              send: <b>{sendWith === "enter" ? comboLabel("Enter") : comboLabel("Mod+Enter")}</b>,
              newLine: <b>{sendWith === "enter" ? comboLabel("Shift+Enter") : comboLabel("Enter")}</b>,
            }}
          />
        </p>
        <p className="ml-auto flex shrink-0 items-center gap-1 font-bold text-emerald-600 dark:text-emerald-400">
          <LockKeyholeIcon className="size-3" /> {error ?? promise}
        </p>
      </div>
    </div>
  );
}
