import {
  ArrowDownIcon,
  CheckIcon,
  CopyIcon,
  CrownIcon,
  PencilIcon,
  RotateCwIcon,
  ShieldIcon,
  SparklesIcon,
  Trash2Icon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
} from "react";
import { MemberRole, type Channel, type Member, type Message, type User } from "@/gen/fuwa/v1/types_pb";
import { deleteMessage, dismissPending, editMessage, loadMessages, run, sendMessage } from "@/fuwa/actions";
import { useInstance } from "@/fuwa/hooks";
import type { PendingMessage } from "@/fuwa/store";
import { Markdown } from "@/components/Markdown";
import { UserAvatar } from "@/components/Icons";
import { displayName, formatDay, formatFull, formatStamp, formatTime, hueOf, sameDay, toDate } from "@/lib/format";
import { cn } from "@/lib/utils";

/** Messages from one person closer together than this share a header. */
const GROUP_GAP_MS = 7 * 60 * 1000;
const EMPTY: never[] = [];

type Row =
  | { kind: "day"; key: string; date: Date }
  | { kind: "message"; key: string; message: Message; first: boolean; date: Date }
  | { kind: "pending"; key: string; pending: PendingMessage; first: boolean };

export type MessageListHandle = { editLast: () => void };

export const MessageList = forwardRef<
  MessageListHandle,
  { instanceKey: string; serverId: string; channel: Channel; manager: boolean }
>(function MessageList({ instanceKey, serverId, channel, manager }, ref) {
  const inst = useInstance(instanceKey);
  const state = inst?.messages[channel.id];
  const items = state?.items ?? EMPTY;
  const pending = inst?.pending[channel.id] ?? EMPTY;
  const members = inst?.members[serverId] ?? EMPTY;
  const users = inst?.users;
  const me = inst?.me;
  const [editing, setEditing] = useState<string | null>(null);

  useImperativeHandle(ref, () => ({
    editLast() {
      const mine = [...items].reverse().find((m) => m.authorId === me?.id);
      if (mine) setEditing(mine.id);
    },
  }));

  useEffect(() => {
    run(loadMessages(instanceKey, serverId, channel.id)).catch(() => {});
  }, [instanceKey, serverId, channel.id]);

  const memberById = useMemo(() => new Map(members.map((m) => [m.user?.id ?? "", m])), [members]);

  const rows = useMemo(() => {
    const out: Row[] = [];
    let prev: { author: string; at: Date } | null = null;
    for (const message of items) {
      const date = toDate(message.createdAt);
      if (!prev || !sameDay(prev.at, date)) {
        out.push({ kind: "day", key: `day-${date.toDateString()}`, date });
        prev = null;
      }
      const first = !prev || prev.author !== message.authorId || date.getTime() - prev.at.getTime() > GROUP_GAP_MS;
      out.push({ kind: "message", key: message.id, message, first, date });
      prev = { author: message.authorId, at: date };
    }
    for (const p of pending) {
      const first = !prev || prev.author !== me?.id || p.createdAt - prev.at.getTime() > GROUP_GAP_MS;
      out.push({ kind: "pending", key: p.nonce, pending: p, first });
      prev = { author: me?.id ?? "", at: new Date(p.createdAt) };
    }
    return out;
  }, [items, pending, me?.id]);

  // ── Scrolling: stick to the bottom while you're there, keep your place when older messages load above.
  const scroller = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const fromBottom = useRef(0);
  const [showJump, setShowJump] = useState(false);
  const [missed, setMissed] = useState(0);
  const lastCount = useRef(0);
  const firstId = useRef<string | undefined>(undefined);

  /** Ids already on screen when the channel opened, so only new arrivals animate in. */
  const initial = useRef<Set<string> | null>(null);
  useEffect(() => {
    initial.current = null;
    atBottom.current = true;
    setMissed(0);
    setShowJump(false);
  }, [channel.id]);
  if (initial.current === null && state && !state.loading) initial.current = new Set(items.map((m) => m.id));

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const prepended = firstId.current !== undefined && items[0]?.id !== firstId.current && items.length > lastCount.current;
    if (atBottom.current) {
      el.scrollTop = el.scrollHeight;
    } else if (prepended) {
      el.scrollTop = el.scrollHeight - fromBottom.current;
    } else if (rows.length > lastCount.current) {
      setMissed((n) => n + (items.length - lastCount.current));
    }
    firstId.current = items[0]?.id;
    lastCount.current = items.length;
  }, [rows, items]);

  const onScroll = useCallback(() => {
    const el = scroller.current;
    if (!el) return;
    fromBottom.current = el.scrollHeight - el.scrollTop;
    const bottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
    atBottom.current = bottom;
    setShowJump(!bottom && el.scrollHeight - el.scrollTop - el.clientHeight > 400);
    if (bottom) setMissed(0);
    if (el.scrollTop < 300 && state?.hasMore && !state.loading) {
      run(loadMessages(instanceKey, serverId, channel.id, true)).catch(() => {});
    }
  }, [instanceKey, serverId, channel.id, state?.hasMore, state?.loading]);

  const jump = () => {
    const el = scroller.current;
    if (!el) return;
    atBottom.current = true;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
    setMissed(0);
  };

  const beginning = state && !state.loading && !state.hasMore;

  return (
    <div className="relative min-h-0 flex-1">
      <div ref={scroller} onScroll={onScroll} className="scroll-thin h-full overflow-y-auto [overflow-anchor:none]">
        <div className="flex min-h-full flex-col justify-end pb-3">
          {state?.loading && items.length > 0 && <Skeleton rows={2} />}
          {beginning && <Beginning channel={channel} />}
          {!state && <Skeleton rows={6} />}
          {state?.loading && items.length === 0 && <Skeleton rows={6} />}
          <AnimatePresence initial={false}>
            {rows.map((row) => {
              if (row.kind === "day") return <DayDivider key={row.key} date={row.date} />;
              if (row.kind === "pending")
                return (
                  <PendingRow
                    key={row.key}
                    pending={row.pending}
                    first={row.first}
                    me={me ?? undefined}
                    member={memberById.get(me?.id ?? "")}
                    onRetry={() => {
                      dismissPending(instanceKey, channel.id, row.pending.nonce);
                      run(sendMessage(instanceKey, serverId, channel.id, row.pending.content)).catch(() => {});
                    }}
                    onDismiss={() => dismissPending(instanceKey, channel.id, row.pending.nonce)}
                  />
                );
              const author = memberById.get(row.message.authorId)?.user ?? users?.[row.message.authorId];
              return (
                <MessageRow
                  key={row.key}
                  message={row.message}
                  first={row.first}
                  date={row.date}
                  author={author}
                  member={memberById.get(row.message.authorId)}
                  mine={row.message.authorId === me?.id}
                  mentionsMe={!!me && row.message.authorId !== me.id && mentions(row.message.content, me.username)}
                  canDelete={manager || row.message.authorId === me?.id}
                  animate={!initial.current?.has(row.message.id)}
                  editing={editing === row.message.id}
                  onEdit={() => setEditing(row.message.id)}
                  onCancelEdit={() => setEditing(null)}
                  onSave={async (content) => {
                    await run(editMessage(instanceKey, serverId, channel.id, row.message.id, content));
                    setEditing(null);
                  }}
                  onDelete={() => run(deleteMessage(instanceKey, serverId, channel.id, row.message.id))}
                />
              );
            })}
          </AnimatePresence>
        </div>
      </div>
      <AnimatePresence>
        {(showJump || missed > 0) && (
          <motion.button
            type="button"
            onClick={jump}
            initial={{ opacity: 0, y: 16, scale: 0.9 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 16, scale: 0.9 }}
            transition={{ type: "spring", stiffness: 500, damping: 30 }}
            className="absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-2 rounded-full bg-primary px-4 py-2 text-sm font-bold text-primary-foreground shadow-lg"
          >
            <ArrowDownIcon className="size-4 animate-bounce" />
            {missed > 0 ? `${missed} new ${missed === 1 ? "message" : "messages"}` : "Jump to present"}
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
});

function mentions(content: string, username: string) {
  return new RegExp(`(^|[^\\w@])@${username.replace(/[.]/g, "\\.")}\\b`, "i").test(content);
}

function DayDivider({ date }: { date: Date }) {
  return (
    <div role="separator" className="my-3 flex items-center gap-3 px-4 text-xs font-bold text-muted-foreground">
      <span className="h-px flex-1 bg-border" />
      {formatDay(date)}
      <span className="h-px flex-1 bg-border" />
    </div>
  );
}

function Beginning({ channel }: { channel: Channel }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.5, ease: [0.22, 1, 0.36, 1] }}
      className="px-4 pt-10 pb-4"
    >
      <span className="float mb-4 grid size-16 place-items-center rounded-full bg-primary/15 text-primary">
        <SparklesIcon className="size-8" />
      </span>
      <h2 className="text-2xl font-extrabold sm:text-3xl">Welcome to #{channel.name}</h2>
      <p className="mt-1 text-muted-foreground">
        This is the start of #{channel.name}.{channel.topic ? ` ${channel.topic}` : ""}
      </p>
    </motion.div>
  );
}

function Skeleton({ rows }: { rows: number }) {
  return (
    <div className="flex flex-col gap-5 px-4 py-4">
      {Array.from({ length: rows }, (_, n) => (
        <div key={n} className="flex gap-3" style={{ opacity: 1 - n * 0.12 }}>
          <div className="shimmer size-10 shrink-0 rounded-full" />
          <div className="flex flex-1 flex-col gap-2 pt-1">
            <div className="shimmer h-3.5 w-32 rounded" />
            <div className="shimmer h-3.5 rounded" style={{ width: `${40 + ((n * 37) % 50)}%` }} />
          </div>
        </div>
      ))}
    </div>
  );
}

const hue = (id: string) => ({ "--h": hueOf(id) }) as CSSProperties;

function AuthorName({ user, member }: { user: User | undefined; member: Member | undefined }) {
  const role = member?.role;
  return (
    <span className="inline-flex min-w-0 items-center gap-1">
      <span className="name-tint truncate font-bold" style={hue(user?.id ?? "")}>
        {member?.nickname || displayName(user)}
      </span>
      {role === MemberRole.OWNER && <CrownIcon aria-label="Owner" className="size-3.5 shrink-0 text-amber-400" />}
      {role === MemberRole.ADMIN && <ShieldIcon aria-label="Admin" className="size-3.5 shrink-0 text-primary" />}
    </span>
  );
}

const enter = { initial: { opacity: 0, y: 12, scale: 0.98 }, animate: { opacity: 1, y: 0, scale: 1 } };

function MessageRow({
  message,
  first,
  date,
  author,
  member,
  mine,
  mentionsMe,
  canDelete,
  animate,
  editing,
  onEdit,
  onCancelEdit,
  onSave,
  onDelete,
}: {
  message: Message;
  first: boolean;
  date: Date;
  author: User | undefined;
  member: Member | undefined;
  mine: boolean;
  mentionsMe: boolean;
  canDelete: boolean;
  animate: boolean;
  editing: boolean;
  onEdit: () => void;
  onCancelEdit: () => void;
  onSave: (content: string) => Promise<void>;
  onDelete: () => Promise<void>;
}) {
  const [confirming, setConfirming] = useState(false);
  const [copied, setCopied] = useState(false);
  const edited = !!message.editedAt;
  return (
    <motion.div
      layout="position"
      {...(animate ? enter : {})}
      exit={{ opacity: 0, height: 0, transition: { duration: 0.2 } }}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      className={cn(
        "message-row group relative flex gap-3 px-4",
        first ? "mt-3 pt-1 pb-0.5" : "py-0.5",
        mentionsMe && "mention-me",
        animate && mine && "landed",
      )}
    >
      <div className="w-10 shrink-0">
        {first ? (
          <UserAvatar user={author} className="mt-0.5" />
        ) : (
          <time className="gutter-time block pt-1 text-right text-[0.65rem] text-muted-foreground tabular-nums" dateTime={date.toISOString()} title={formatFull(date)}>
            {formatTime(date)}
          </time>
        )}
      </div>
      <div className="min-w-0 flex-1">
        {first && (
          <div className="flex items-baseline gap-2">
            <AuthorName user={author} member={member} />
            <time className="shrink-0 text-xs text-muted-foreground" dateTime={date.toISOString()} title={formatFull(date)}>
              {formatStamp(date)}
            </time>
          </div>
        )}
        {editing ? (
          <EditBox initial={message.content} onCancel={onCancelEdit} onSave={onSave} />
        ) : (
          <div className="text-[0.95rem] leading-relaxed">
            <Markdown className="chat">{message.content}</Markdown>
            {edited && (
              <span className="text-[0.7rem] text-muted-foreground" title={formatFull(toDate(message.editedAt))}>
                {" "}
                (edited)
              </span>
            )}
          </div>
        )}
      </div>
      {!editing && (
        <div className="message-tools absolute -top-3 right-4 z-10 flex items-center gap-0.5 rounded-xl border bg-card p-0.5 shadow-md">
          {confirming ? (
            <>
              <span className="px-2 text-xs font-bold">Delete?</span>
              <ToolButton label="Delete" danger onClick={() => onDelete().catch(() => setConfirming(false))}>
                <CheckIcon />
              </ToolButton>
              <ToolButton label="Keep" onClick={() => setConfirming(false)}>
                <XIcon />
              </ToolButton>
            </>
          ) : (
            <>
              <ToolButton
                label={copied ? "Copied" : "Copy text"}
                onClick={() => {
                  void navigator.clipboard?.writeText(message.content);
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1200);
                }}
              >
                {copied ? <CheckIcon className="text-primary" /> : <CopyIcon />}
              </ToolButton>
              {mine && (
                <ToolButton label="Edit" onClick={onEdit}>
                  <PencilIcon />
                </ToolButton>
              )}
              {canDelete && (
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
}

function ToolButton({
  label,
  danger = false,
  onClick,
  children,
}: {
  label: string;
  danger?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={cn(
        "grid size-8 place-items-center rounded-lg text-muted-foreground transition hover:scale-110 [&_svg]:size-4",
        danger ? "hover:bg-destructive/15 hover:text-destructive" : "hover:bg-muted hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

function EditBox({ initial, onCancel, onSave }: { initial: string; onCancel: () => void; onSave: (c: string) => Promise<void> }) {
  const [text, setText] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const box = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);
  useEffect(() => {
    const el = box.current;
    el?.focus();
    el?.setSelectionRange(el.value.length, el.value.length);
  }, []);
  async function save() {
    const content = text.trim();
    if (!content || content === initial) return onCancel();
    try {
      await onSave(content);
    } catch (e) {
      setError((e as Error).message);
    }
  }
  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Escape") onCancel();
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void save();
    }
  }
  return (
    <div className="mt-1">
      <textarea
        ref={box}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
        className="composer w-full resize-none rounded-xl border bg-card px-3 py-2 text-[0.95rem] leading-6 outline-none"
      />
      <p className="text-xs text-muted-foreground">
        {error ? <span className="text-destructive">{error} · </span> : null}
        Escape to <button type="button" className="font-bold text-primary hover:underline" onClick={onCancel}>cancel</button> · Enter to{" "}
        <button type="button" className="font-bold text-primary hover:underline" onClick={() => void save()}>save</button>
      </p>
    </div>
  );
}

function PendingRow({
  pending,
  first,
  me,
  member,
  onRetry,
  onDismiss,
}: {
  pending: PendingMessage;
  first: boolean;
  me: User | undefined;
  member: Member | undefined;
  onRetry: () => void;
  onDismiss: () => void;
}) {
  return (
    <motion.div
      layout="position"
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: pending.failed ? 1 : 0.55, y: 0 }}
      exit={{ opacity: 0 }}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      className={cn("flex gap-3 px-4", first ? "mt-3 pt-1 pb-0.5" : "py-0.5")}
    >
      <div className="w-10 shrink-0">{first && <UserAvatar user={me} className="mt-0.5" />}</div>
      <div className="min-w-0 flex-1">
        {first && (
          <div className="flex items-baseline gap-2">
            <AuthorName user={me} member={member} />
            <span className="text-xs text-muted-foreground">sending…</span>
          </div>
        )}
        <div className={cn("text-[0.95rem] leading-relaxed", pending.failed && "text-destructive")}>
          <Markdown className="chat">{pending.content}</Markdown>
        </div>
        {pending.failed && (
          <p className="mt-1 flex flex-wrap items-center gap-2 text-xs">
            <span className="text-destructive first-letter:uppercase">{pending.failed}.</span>
            <button type="button" onClick={onRetry} className="inline-flex items-center gap-1 font-bold text-primary hover:underline">
              <RotateCwIcon className="size-3" /> Retry
            </button>
            <button type="button" onClick={onDismiss} className="font-bold text-muted-foreground hover:underline">
              Dismiss
            </button>
          </p>
        )}
      </div>
    </motion.div>
  );
}
