import {
  ArrowDownIcon,
  ArrowRightIcon,
  CheckIcon,
  CopyIcon,
  CrownIcon,
  FingerprintIcon,
  PencilIcon,
  RotateCwIcon,
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
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { MessageKind, Permission, type Channel, type Member, type Message, type User } from "@/gen/fuwa/v1/types_pb";
import { deleteMessage, dismissPending, editMessage, loadMessages, run, sendMessage } from "@/fuwa/actions";
import { useAccess, useInstance, useRoles } from "@/fuwa/hooks";
import type { PendingMessage } from "@/fuwa/store";
import { sendsMessage } from "@/components/chat/Composer";
import { Mention, remarkMentions, ServerLookProvider, useRoleColor, useServerLook, type ServerLook } from "@/components/chat/mentions";
import { Markdown, type MarkdownExtension } from "@/components/Markdown";
import { RoleName } from "@/components/RoleName";
import { UserAvatar } from "@/components/Icons";
import { ProfilePopover } from "@/components/ProfilePopover";
import { displayName, formatDay, formatFull, formatStamp, formatTime, hueOf, sameDay, toDate } from "@/lib/format";
import { comboLabel } from "@/lib/keybinds";
import { pingsMe, useNotificationSettings } from "@/lib/notifications";
import { hasIn } from "@/lib/permissions";
import { usePrefs, type MessageDisplay } from "@/lib/prefs";
import { copy } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Messages from one person closer together than this share a header. */
const GROUP_GAP_MS = 7 * 60 * 1000;
const EMPTY: never[] = [];

type Row =
  | { kind: "day"; key: string; date: Date }
  | { kind: "message"; key: string; message: Message; first: boolean; date: Date }
  | { kind: "join"; key: string; message: Message; date: Date }
  | { kind: "pending"; key: string; pending: PendingMessage; first: boolean };

export type MessageListHandle = { editLast: () => void };

export const MessageList = forwardRef<
  MessageListHandle,
  { instanceKey: string; serverId: string; channel: Channel }
>(function MessageList({ instanceKey, serverId, channel }, ref) {
  const inst = useInstance(instanceKey);
  const access = useAccess(instanceKey, serverId);
  const manager = hasIn(access, channel.id, Permission.MANAGE_MESSAGES);
  const canSend = hasIn(access, channel.id, Permission.SEND_MESSAGES);
  const roles = useRoles(instanceKey, serverId);
  const state = inst?.messages[channel.id];
  const items = state?.items ?? EMPTY;
  const pending = inst?.pending[channel.id] ?? EMPTY;
  const members = inst?.members[serverId] ?? EMPTY;
  const users = inst?.users;
  const me = inst?.me;
  const [editing, setEditing] = useState<string | null>(null);
  const display = usePrefs((p) => p.messageDisplay);
  const developer = usePrefs((p) => p.developerMode);
  const suppressEveryone = useNotificationSettings(instanceKey, serverId)?.suppressEveryone ?? false;
  // Times follow the clock setting; reading it here re-renders the rows when it changes.
  usePrefs((p) => p.clock);

  useImperativeHandle(ref, () => ({
    editLast() {
      const mine = [...items].reverse().find((m) => m.authorId === me?.id && m.kind === MessageKind.UNSPECIFIED);
      if (mine) setEditing(mine.id);
    },
  }));

  useEffect(() => {
    run(loadMessages(instanceKey, serverId, channel.id)).catch(() => {});
  }, [instanceKey, serverId, channel.id]);

  const memberById = useMemo(() => new Map(members.map((m) => [m.user?.id ?? "", m])), [members]);
  const myRoleIds = memberById.get(me?.id ?? "")?.roleIds;
  const ownerId = inst?.servers.find((s) => s.id === serverId)?.ownerId ?? "";
  const look = useMemo<ServerLook>(
    () => ({
      instanceKey,
      ownerId,
      roles,
      members,
      me: me ? { id: me.id, username: me.username, roleIds: myRoleIds ?? [] } : undefined,
    }),
    [instanceKey, ownerId, roles, members, me, myRoleIds],
  );

  const rows = useMemo(() => {
    const out: Row[] = [];
    let prev: { author: string; at: Date } | null = null;
    for (const message of items) {
      const date = toDate(message.createdAt);
      if (!prev || !sameDay(prev.at, date)) {
        out.push({ kind: "day", key: `day-${date.toDateString()}`, date });
        prev = null;
      }
      if (message.kind === MessageKind.MEMBER_JOINED) {
        out.push({ kind: "join", key: message.id, message, date });
        // The next message starts a run of its own.
        prev = { author: "", at: date };
        continue;
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
    <ServerLookProvider value={look}>
    <div className="relative min-h-0 flex-1">
      <div ref={scroller} onScroll={onScroll} className="scroll-thin h-full overflow-y-auto [overflow-anchor:none]">
        <motion.div
          initial={{ opacity: 0, y: 12 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.35, ease: [0.22, 1, 0.36, 1] }}
          className="flex min-h-full flex-col justify-end pb-3"
        >
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
                    display={display}
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
              if (row.kind === "join")
                return (
                  <JoinRow
                    key={row.key}
                    message={row.message}
                    date={row.date}
                    author={author}
                    member={memberById.get(row.message.authorId)}
                    instanceKey={instanceKey}
                    mine={row.message.authorId === me?.id}
                    animate={!initial.current?.has(row.message.id)}
                    canDelete={manager}
                    onWave={canSend ? () => run(sendMessage(instanceKey, serverId, channel.id, `👋 @${author?.username ?? ""}`)) : undefined}
                    onDelete={() => run(deleteMessage(instanceKey, serverId, channel.id, row.message.id))}
                  />
                );
              return (
                <MessageRow
                  key={row.key}
                  message={row.message}
                  first={row.first}
                  display={display}
                  developer={developer}
                  date={row.date}
                  author={author}
                  member={memberById.get(row.message.authorId)}
                  mine={row.message.authorId === me?.id}
                  mentionsMe={!!inst && pingsMe(inst, serverId, row.message, suppressEveryone)}
                  instanceKey={instanceKey}
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
        </motion.div>
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
    </ServerLookProvider>
  );
});

export function DayDivider({ date }: { date: Date }) {
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

/** Someone's name in chat, in their role's color, with a crown for the server's owner. */
export function AuthorName({ user, member }: { user: User | undefined; member: Member | undefined }) {
  const { ownerId } = useServerLook();
  const color = useRoleColor(member);
  return (
    <span className="inline-flex min-w-0 items-center gap-1">
      <RoleName id={user?.id ?? ""} name={member?.nickname || displayName(user)} color={color} />
      {!!ownerId && user?.id === ownerId && <CrownIcon aria-label="Owner" className="size-3.5 shrink-0 text-amber-400" />}
    </span>
  );
}

const enter = { initial: { opacity: 0, y: 12, scale: 0.98 }, animate: { opacity: 1, y: 0, scale: 1 } };

/**
 * The parts of a message line every message shares: avatar or time, name and
 * body. Cozy puts the avatar and a header over the first of a run; compact
 * puts the time and name in front of every message, on one line.
 */
export function MessageLine({
  display,
  first,
  author,
  member,
  date,
  status,
  instanceKey,
  children,
}: {
  display: MessageDisplay;
  first: boolean;
  author: User | undefined;
  member: Member | undefined;
  /** When it was sent; missing while it's still sending. */
  date?: Date;
  /** Shown instead of the time, like "sending…". */
  status?: string;
  /** Where the author's profile card loads from; without it, names don't open one. */
  instanceKey?: string;
  children: React.ReactNode;
}) {
  const card = (child: React.ReactElement) =>
    instanceKey && author ? (
      <ProfilePopover instanceKey={instanceKey} user={author} member={member}>
        {child}
      </ProfilePopover>
    ) : (
      child
    );
  if (display === "compact")
    return (
      <div className="chat-text min-w-0 flex-1 leading-relaxed">
        {date ? (
          <time className="mr-2 inline-block w-[4.6em] text-right text-[0.7em] text-muted-foreground tabular-nums" dateTime={date.toISOString()} title={formatFull(date)}>
            {formatTime(date)}
          </time>
        ) : (
          <span className="mr-2 inline-block w-[4.6em] text-right text-[0.7em] text-muted-foreground">{status}</span>
        )}
        <span className="mr-1.5 inline-flex max-w-[40%] align-bottom">
          {card(
            <button type="button" className="min-w-0 text-left hover:underline">
              <AuthorName user={author} member={member} />
            </button>,
          )}
        </span>
        {children}
      </div>
    );
  return (
    <>
      <div className="w-10 shrink-0">
        {first ? (
          card(
            <button type="button" aria-label="Open profile" className="mt-0.5 block rounded-full transition hover:brightness-110 active:scale-95">
              <UserAvatar user={author} />
            </button>,
          )
        ) : date ? (
          <time className="gutter-time -ml-3 block pt-1 text-right text-[0.625rem] whitespace-nowrap text-muted-foreground tabular-nums" dateTime={date.toISOString()} title={formatFull(date)}>
            {formatTime(date)}
          </time>
        ) : null}
      </div>
      <div className="min-w-0 flex-1">
        {first && (
          <div className="flex items-baseline gap-2">
            {card(
              <button type="button" className="min-w-0 text-left hover:underline">
                <AuthorName user={author} member={member} />
              </button>,
            )}
            {date ? (
              <time className="shrink-0 text-xs text-muted-foreground" dateTime={date.toISOString()} title={formatFull(date)}>
                {formatStamp(date)}
              </time>
            ) : (
              <span className="text-xs text-muted-foreground">{status}</span>
            )}
          </div>
        )}
        <div className="chat-text leading-relaxed">{children}</div>
      </div>
    </>
  );
}

const CHAT: MarkdownExtension = { remarkPlugins: [remarkMentions], components: { "fuwa-mention": Mention } };

/** A message's text, mentions and all; in compact display its first paragraph runs on after the name. */
export function MessageBody({ content, display, className }: { content: string; display: MessageDisplay; className?: string }) {
  return (
    <Markdown className={cn("chat", display === "compact" && "inline-first", className)} extension={CHAT}>
      {content}
    </Markdown>
  );
}

function MessageRow({
  message,
  first,
  display,
  developer,
  date,
  author,
  member,
  mine,
  mentionsMe,
  instanceKey,
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
  display: MessageDisplay;
  developer: boolean;
  date: Date;
  author: User | undefined;
  member: Member | undefined;
  mine: boolean;
  mentionsMe: boolean;
  instanceKey: string;
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
        first && "first",
        display === "compact" && "compact",
        mentionsMe && "mention-me",
        animate && mine && "landed",
      )}
    >
      <MessageLine display={display} first={first} author={author} member={member} date={date} instanceKey={instanceKey}>
        {editing ? (
          <EditBox initial={message.content} onCancel={onCancelEdit} onSave={onSave} />
        ) : (
          <>
            <MessageBody content={message.content} display={display} />
            {edited && (
              <span className="text-[0.7rem] text-muted-foreground" title={formatFull(toDate(message.editedAt))}>
                {" "}
                (edited)
              </span>
            )}
          </>
        )}
      </MessageLine>
      {!editing && (
        <div className="message-tools absolute -top-3 right-4 z-10 flex items-center gap-0.5 rounded-xl border bg-card p-0.5 shadow-md">
          {confirming ? (
            <motion.span
              key="confirm"
              initial={{ opacity: 0, x: 8 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ type: "spring", stiffness: 600, damping: 32 }}
              className="flex items-center gap-0.5"
            >
              <span className="px-2 text-xs font-bold text-destructive">Delete?</span>
              <ToolButton label="Delete" danger onClick={() => onDelete().catch(() => setConfirming(false))}>
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
                  void navigator.clipboard?.writeText(message.content);
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
              {developer && (
                <ToolButton label="Copy message ID" onClick={() => copy(message.id, "message ID")}>
                  <FingerprintIcon />
                </ToolButton>
              )}
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

/** Ways to say someone joined, picked by who they are so each join keeps its line. */
const JOIN_LINES: ((name: ReactNode) => ReactNode)[] = [
  (n) => <>{n} just landed.</>,
  (n) => <>Welcome, {n}. Say hi!</>,
  (n) => <>{n} joined the party.</>,
  (n) => <>A wild {n} appeared.</>,
  (n) => <>{n} hopped into the server.</>,
  (n) => <>Everyone, welcome {n}!</>,
  (n) => <>{n} is here. Glad you made it!</>,
  (n) => <>Good to see you, {n}.</>,
];

export const joinLine = (userId: string, name: ReactNode) => JOIN_LINES[hueOf(userId) % JOIN_LINES.length]!(name);

/** Waves already sent this session, so the button remembers. */
const waved = new Set<string>();

/** "Someone joined", with a button to wave at them. */
function JoinRow({
  message,
  date,
  author,
  member,
  instanceKey,
  mine,
  animate,
  canDelete,
  onWave,
  onDelete,
}: {
  message: Message;
  date: Date;
  author: User | undefined;
  member: Member | undefined;
  instanceKey: string;
  mine: boolean;
  animate: boolean;
  canDelete: boolean;
  /** Unset where you can't send messages, such as before agreeing to the rules. */
  onWave?: () => Promise<void>;
  onDelete: () => Promise<void>;
}) {
  const [done, setDone] = useState(waved.has(message.id));
  const [waving, setWaving] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const color = useRoleColor(member);
  const name = (
    <ProfilePopover instanceKey={instanceKey} user={author} member={member}>
      <button type="button" className="inline-flex align-bottom hover:underline">
        <RoleName id={message.authorId} name={member?.nickname || displayName(author)} color={color} />
      </button>
    </ProfilePopover>
  );
  return (
    <motion.div
      layout="position"
      {...(animate ? enter : {})}
      exit={{ opacity: 0, height: 0, transition: { duration: 0.2 } }}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      className="message-row join-row group relative flex items-center gap-3 px-4 py-1.5"
    >
      <span className="grid w-10 shrink-0 place-items-center">
        <ArrowRightIcon className="size-4 text-emerald-500 transition-transform duration-300 group-hover:translate-x-1" />
      </span>
      <p className="min-w-0 flex-1 text-[0.94rem] text-muted-foreground">
        {joinLine(message.authorId, name)}{" "}
        <time className="text-xs whitespace-nowrap" dateTime={date.toISOString()} title={formatFull(date)}>
          {formatStamp(date)}
        </time>
      </p>
      {!mine && author && onWave && (
        <motion.button
          type="button"
          disabled={done || waving}
          whileTap={{ scale: 0.9 }}
          onClick={async () => {
            setWaving(true);
            try {
              await onWave();
              waved.add(message.id);
              setDone(true);
            } finally {
              setWaving(false);
            }
          }}
          className={cn(
            "group/wave flex shrink-0 items-center gap-1.5 rounded-full border px-3 py-1 text-xs font-bold transition-colors",
            done ? "border-transparent bg-emerald-500/10 text-emerald-600 dark:text-emerald-400" : "hover:border-primary/40 hover:bg-primary/10 hover:text-primary",
          )}
        >
          <motion.span
            aria-hidden
            animate={waving || done ? { rotate: [0, 22, -10, 22, -6, 0] } : { rotate: 0 }}
            transition={{ duration: 0.8 }}
            style={{ originX: 0.7, originY: 0.8 }}
            className="inline-block group-hover/wave:animate-[wave_0.9s_ease-in-out]"
          >
            👋
          </motion.span>
          {done ? "Waved" : "Wave"}
        </motion.button>
      )}
      {canDelete && (
        <div className="message-tools absolute -top-3 right-4 z-10 flex items-center gap-0.5 rounded-xl border bg-card p-0.5 shadow-md">
          {confirming ? (
            <span className="flex items-center gap-0.5">
              <span className="px-2 text-xs font-bold text-destructive">Delete?</span>
              <ToolButton label="Delete" danger onClick={() => onDelete().catch(() => setConfirming(false))}>
                <CheckIcon />
              </ToolButton>
              <ToolButton label="Keep" onClick={() => setConfirming(false)}>
                <XIcon />
              </ToolButton>
            </span>
          ) : (
            <ToolButton label="Delete" danger onClick={() => setConfirming(true)}>
              <Trash2Icon />
            </ToolButton>
          )}
        </div>
      )}
    </motion.div>
  );
}

export function ToolButton({
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

export function EditBox({ initial, onCancel, onSave }: { initial: string; onCancel: () => void; onSave: (c: string) => Promise<void> }) {
  const [text, setText] = useState(initial);
  const sendWith = usePrefs((p) => p.sendWith);
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
    if (sendsMessage(e, sendWith)) {
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
        Escape to <button type="button" className="font-bold text-primary hover:underline" onClick={onCancel}>cancel</button> ·{" "}
        {sendWith === "enter" ? comboLabel("Enter") : comboLabel("Mod+Enter")} to{" "}
        <button type="button" className="font-bold text-primary hover:underline" onClick={() => void save()}>save</button>
      </p>
    </div>
  );
}

function PendingRow({
  pending,
  first,
  display,
  me,
  member,
  onRetry,
  onDismiss,
}: {
  pending: PendingMessage;
  first: boolean;
  display: MessageDisplay;
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
      className={cn("message-row flex gap-3 px-4", first && "first", display === "compact" && "compact")}
    >
      <MessageLine display={display} first={first} author={me} member={member} status="sending…">
        <MessageBody content={pending.content} display={display} className={cn(pending.failed && "text-destructive")} />
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
      </MessageLine>
    </motion.div>
  );
}
