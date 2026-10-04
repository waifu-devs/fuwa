import {
  ArrowDownIcon,
  ArrowRightIcon,
  CheckIcon,
  CopyIcon,
  CrownIcon,
  FingerprintIcon,
  MessageSquareReplyIcon,
  PencilIcon,
  RotateCwIcon,
  ShieldAlertIcon,
  ShieldIcon,
  SparklesIcon,
  TimerIcon,
  Trash2Icon,
  UserXIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import {
  forwardRef,
  memo,
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
import {
  AutoModTrigger,
  MessageKind,
  Permission,
  type Channel,
  type Emoji,
  type Member,
  type Message,
  type MessageWebhook,
  type SharedServer,
  type User,
} from "@/gen/fuwa/v1/types_pb";
import { blockFromChannel, deleteMessage, dismissPending, editMessage, loadMessages, run, sendMessage } from "@/fuwa/actions";
import { useAccess, useRoles } from "@/fuwa/hooks";
import { threadKey, useFuwa, type PendingMessage } from "@/fuwa/store";
import { AlsoSentNote, RepliesRow } from "@/components/chat/Threads";
import { useThreadOpener } from "@/lib/threads";
import { sendsMessage } from "@/components/chat/Composer";
import { Mention, MessageEmojis, remarkMentions, ServerLookProvider, useRoleColor, useServerLook, type ServerLook } from "@/components/chat/mentions";
import { Markdown, type MarkdownExtension } from "@/components/Markdown";
import { EMOJI_TOKEN, onlyEmoji } from "@/lib/emoji";
import { encodeEmoji, useCatalog } from "@/lib/emoji-catalog";
import { RoleName } from "@/components/RoleName";
import { UserAvatar } from "@/components/Icons";
import { ProfilePopover } from "@/components/ProfilePopover";
import { Embeds } from "@/components/chat/Embeds";
import { Attachments, PendingFiles } from "@/components/chat/Attachments";
import { AppBadge } from "@/components/AppBadge";
import { ServerTag, SharedNote } from "@/components/chat/Shared";
import { displayName, isAgent, formatDuration, formatDay, formatFull, formatStamp, formatTime, hueOf, sameDay, toDate } from "@/lib/format";
import { comboLabel } from "@/lib/keybinds";
import { pingsUser, useNotificationSettings } from "@/lib/notifications";
import { has, hasIn } from "@/lib/permissions";
import { foreignServer } from "@/lib/shared";
import { usePrefs, type Clock, type MessageDisplay } from "@/lib/prefs";
import { copy, toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Messages from one person closer together than this share a header. */
const GROUP_GAP_MS = 7 * 60 * 1000;
const EMPTY: never[] = [];

type Row =
  | { kind: "day"; key: string; date: Date }
  | { kind: "message"; key: string; message: Message; first: boolean; date: Date }
  | { kind: "join"; key: string; message: Message; date: Date }
  | { kind: "automod"; key: string; message: Message; date: Date }
  | { kind: "pending"; key: string; pending: PendingMessage; first: boolean };

export type MessageListHandle = {
  editLast: () => void;
  /** Scrolls to a message and lights it up. False when it isn't loaded here. */
  jumpTo: (id: string) => boolean;
};

/** Rows drawn when a channel opens; older ones already loaded come in as you scroll up. */
const FIRST_ROWS = 80;
/** How many more rows each scroll to the top reveals before asking the server for older messages. */
const MORE_ROWS = 80;

/*
 * Rows keep the same props while their message is unchanged, so memoized
 * rows skip rendering when something else in the channel changes. A
 * message's date and a webhook's author are made once per (immutable) object.
 */
const dates = new WeakMap<Message, Date>();
const dateOf = (m: Message) => {
  let d = dates.get(m);
  if (!d) dates.set(m, (d = toDate(m.createdAt)));
  return d;
};
const webhookAuthors = new WeakMap<MessageWebhook, User>();
const webhookAuthorOf = (w: MessageWebhook) => {
  let u = webhookAuthors.get(w);
  if (!u) webhookAuthors.set(w, (u = webhookAuthor(w)));
  return u;
};

/** What a row can do to its message, the same object for the whole channel. */
type RowActions = {
  edit: (id: string) => void;
  cancelEdit: () => void;
  save: (id: string, content: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
  wave: (username: string) => Promise<void>;
  retry: (pending: PendingMessage) => void;
  dismiss: (nonce: string) => void;
  /** At a shared channel's home: keeps someone from another server out of it. */
  keepOut: (userId: string, name: string) => Promise<void>;
  /** Opens the thread under a message (starting it with the first reply). */
  thread: (id: string) => void;
};

export const MessageList = forwardRef<
  MessageListHandle,
  {
    instanceKey: string;
    serverId: string;
    channel: Channel;
    /** The replies in the thread under this message, instead of the channel. */
    threadId?: string;
    /** What a thread starts with, in place of the channel's welcome. */
    header?: ReactNode;
  }
>(function MessageList({ instanceKey, serverId, channel, threadId = "", header }, ref) {
  // Where this list's messages are kept: the channel's, or a thread's.
  const at = threadId ? threadKey(threadId) : channel.id;
  // Only the pieces this list draws, so events elsewhere on the instance don't re-render it.
  const state = useFuwa((s) => s.instances[instanceKey]?.messages[at]);
  const pending = useFuwa((s) => s.instances[instanceKey]?.pending[at] ?? EMPTY);
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? EMPTY);
  const emojis = useFuwa((s) => s.instances[instanceKey]?.emojis[serverId] ?? EMPTY);
  const catalog = useCatalog(instanceKey, serverId);
  const otherEmojis = useMemo(
    () => new Map([...catalog.byId].filter(([, c]) => !c.here).map(([id, c]) => [id, c.emoji])),
    [catalog],
  );
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  const me = useFuwa((s) => s.instances[instanceKey]?.me ?? undefined);
  const ownerId = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.ownerId ?? "");
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[serverId] ?? EMPTY);
  const access = useAccess(instanceKey, serverId);
  const manager = hasIn(access, channel.id, Permission.MANAGE_MESSAGES);
  const canSend = hasIn(access, channel.id, Permission.SEND_MESSAGES);
  // Threads go under messages in the channel itself, not under replies, and not in shared channels yet.
  const threads = !threadId && !channel.shared;
  const canStart = threads && hasIn(access, channel.id, Permission.CREATE_THREADS);
  const canReply = threads && canSend;
  const openThread = useThreadOpener();
  // In a shared channel each side moderates its own people: a guest's moderators can't delete the home's, and
  // only the home keeps someone from another server out.
  const guestSide = !!channel.shared && !channel.shared.home;
  const keepsOut = !!channel.shared?.home && has(access, Permission.KICK_MEMBERS);
  const roles = useRoles(instanceKey, serverId);
  const items = state?.items ?? EMPTY;
  const [editing, setEditing] = useState<string | null>(null);
  const display = usePrefs((p) => p.messageDisplay);
  const developer = usePrefs((p) => p.developerMode);
  const suppressEveryone = useNotificationSettings(instanceKey, serverId)?.suppressEveryone ?? false;
  // Times follow the clock setting: a new clock re-renders every row.
  const clock = usePrefs((p) => p.clock);

  useImperativeHandle(ref, () => ({
    editLast() {
      const mine = [...items].reverse().find((m) => m.authorId === me?.id && m.kind === MessageKind.UNSPECIFIED);
      if (mine) setEditing(mine.id);
    },
    jumpTo(id) {
      const index = rowsRef.current.findIndex((r) => r.key === id);
      if (index === -1) return false;
      // Draw the rows down to it first if they're hidden above.
      if (index < skippedRef.current) setHidden(Math.max(0, index - 10));
      atBottom.current = false;
      const light = (tries: number) =>
        requestAnimationFrame(() => {
          const el = scroller.current?.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(id)}"]`);
          if (!el) return tries > 0 && light(tries - 1);
          el.scrollIntoView({ block: "center", behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
          el.classList.remove("jumped");
          void el.offsetWidth;
          el.classList.add("jumped");
        });
      light(5);
      return true;
    },
  }));

  useEffect(() => {
    run(loadMessages(instanceKey, serverId, channel.id, false, threadId)).catch(() => {});
  }, [instanceKey, serverId, channel.id, threadId]);

  const memberById = useMemo(() => new Map(members.map((m) => [m.user?.id ?? "", m])), [members]);
  const myRoleIds = memberById.get(me?.id ?? "")?.roleIds;
  const look = useMemo<ServerLook>(
    () => ({
      instanceKey,
      ownerId,
      roles,
      members,
      emojis,
      otherEmojis,
      me: me ? { id: me.id, username: me.username, roleIds: myRoleIds ?? [] } : undefined,
    }),
    [instanceKey, ownerId, roles, members, emojis, otherEmojis, me, myRoleIds],
  );

  const actions = useMemo<RowActions>(
    () => ({
      edit: setEditing,
      cancelEdit: () => setEditing(null),
      save: async (id, content) => {
        await run(editMessage(instanceKey, serverId, channel.id, id, encodeEmoji(content, catalog)));
        setEditing(null);
      },
      remove: (id) => run(deleteMessage(instanceKey, serverId, channel.id, id)),
      wave: (username) => run(sendMessage(instanceKey, serverId, channel.id, `👋 @${username}`)),
      retry: (p) => {
        dismissPending(instanceKey, at, p.nonce);
        run(sendMessage(instanceKey, serverId, channel.id, p.content, p.files, threadId ? { threadId } : undefined)).catch(() => {});
      },
      dismiss: (nonce) => dismissPending(instanceKey, at, nonce),
      keepOut: async (userId, name) => {
        await run(blockFromChannel(instanceKey, serverId, channel.id, userId, true));
        toast(`${name} can't see #${channel.name} anymore`);
      },
      thread: (id) => openThread?.(id),
    }),
    [instanceKey, serverId, channel.id, channel.name, catalog, at, threadId, openThread],
  );

  const rows = useMemo(() => {
    const out: Row[] = [];
    let prev: { author: string; at: Date } | null = null;
    for (const message of items) {
      const date = dateOf(message);
      if (!prev || !sameDay(prev.at, date)) {
        out.push({ kind: "day", key: `day-${date.toDateString()}`, date });
        prev = null;
      }
      if (message.kind === MessageKind.MEMBER_JOINED || message.kind === MessageKind.AUTO_MOD_ALERT) {
        out.push({ kind: message.kind === MessageKind.MEMBER_JOINED ? "join" : "automod", key: message.id, message, date });
        // The next message starts a run of its own.
        prev = { author: "", at: date };
        continue;
      }
      const author = runKey(message);
      const first = !prev || prev.author !== author || date.getTime() - prev.at.getTime() > GROUP_GAP_MS;
      out.push({ kind: "message", key: message.id, message, first, date });
      prev = { author, at: date };
    }
    for (const p of pending) {
      const first = !prev || prev.author !== me?.id || p.createdAt - prev.at.getTime() > GROUP_GAP_MS;
      out.push({ kind: "pending", key: p.nonce, pending: p, first });
      prev = { author: me?.id ?? "", at: new Date(p.createdAt) };
    }
    return out;
  }, [items, pending, me?.id]);

  // ── Drawing only the latest rows: a long channel opens with FIRST_ROWS, and scrolling up reveals the rest.
  const [hidden, setHidden] = useState<number | null>(null);
  const ready = !!state && !state.loading;
  const skipped = hidden ?? (ready ? Math.max(0, rows.length - FIRST_ROWS) : 0);
  if (hidden === null && ready) setHidden(skipped);
  const shown = skipped ? rows.slice(skipped) : rows;
  const rowCount = useRef(rows.length);
  // What jumpTo reads, kept as of the last render.
  const rowsRef = useRef(rows);
  const skippedRef = useRef(skipped);
  useLayoutEffect(() => {
    rowsRef.current = rows;
    skippedRef.current = skipped;
  }, [rows, skipped]);

  // ── Scrolling: stick to the bottom while you're there, keep your place when older messages load above.
  const scroller = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const fromBottom = useRef(0);
  const [showJump, setShowJump] = useState(false);
  const [missed, setMissed] = useState(0);
  const lastCount = useRef(0);
  const firstKey = useRef<string | undefined>(undefined);
  const lastShown = useRef(0);

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
    // Day dividers keep their key when older messages of the same day come in, so look past them.
    const top = shown.find((r) => r.kind !== "day")?.key;
    const prepended = firstKey.current !== undefined && top !== firstKey.current && shown.length > lastShown.current;
    if (atBottom.current) {
      el.scrollTop = el.scrollHeight;
    } else if (prepended) {
      el.scrollTop = el.scrollHeight - fromBottom.current;
    } else if (items.length > lastCount.current) {
      setMissed((n) => n + (items.length - lastCount.current));
    }
    firstKey.current = top;
    lastShown.current = shown.length;
    rowCount.current = rows.length;
    lastCount.current = items.length;
  }, [shown, items, rows]);

  const onScroll = useCallback(() => {
    const el = scroller.current;
    if (!el) return;
    fromBottom.current = el.scrollHeight - el.scrollTop;
    const bottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
    atBottom.current = bottom;
    setShowJump(!bottom && el.scrollHeight - el.scrollTop - el.clientHeight > 400);
    if (bottom) setMissed(0);
    // Back at the bottom after reading far up: let go of the rows far above, so new messages stay cheap.
    if (bottom && rowCount.current - skipped > FIRST_ROWS + MORE_ROWS) setHidden(rowCount.current - FIRST_ROWS);
    if (el.scrollTop < 300) {
      if (skipped > 0) setHidden(Math.max(0, skipped - MORE_ROWS));
      else if (state?.hasMore && !state.loading) run(loadMessages(instanceKey, serverId, channel.id, true, threadId)).catch(() => {});
    }
  }, [instanceKey, serverId, channel.id, threadId, state?.hasMore, state?.loading, skipped]);

  const jump = () => {
    const el = scroller.current;
    if (!el) return;
    atBottom.current = true;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
    setMissed(0);
  };

  const beginning = state && !state.loading && !state.hasMore && skipped === 0;
  const meId = me?.id;
  const meMember = memberById.get(meId ?? "");

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
          {beginning && (threadId ? header : <Beginning channel={channel} />)}
          {!state && <Skeleton rows={6} />}
          {state?.loading && items.length === 0 && <Skeleton rows={6} />}
          {/* Rows don't animate their layout, so a new arrival needn't re-render every row (the default does). */}
          <AnimatePresence initial={false} presenceAffectsLayout={false}>
            {shown.map((row) => {
              if (row.kind === "day") return <DayDivider key={row.key} date={row.date} />;
              if (row.kind === "pending")
                return (
                  <PendingRow
                    key={row.key}
                    pending={row.pending}
                    first={row.first}
                    display={display}
                    me={me}
                    member={meMember}
                    actions={actions}
                  />
                );
              const author = row.message.webhook
                ? webhookAuthorOf(row.message.webhook)
                : (memberById.get(row.message.authorId)?.user ?? users?.[row.message.authorId]);
              if (row.kind === "automod")
                return (
                  <AutoModAlertRow
                    key={row.key}
                    message={row.message}
                    date={row.date}
                    author={author}
                    member={memberById.get(row.message.authorId)}
                    instanceKey={instanceKey}
                    channelName={channels.find((c) => c.id === row.message.autoMod?.channelId)?.name}
                    animate={!initial.current?.has(row.message.id)}
                    canDelete={manager}
                    actions={actions}
                    clock={clock}
                  />
                );
              if (row.kind === "join")
                return (
                  <JoinRow
                    key={row.key}
                    message={row.message}
                    date={row.date}
                    author={author}
                    member={memberById.get(row.message.authorId)}
                    instanceKey={instanceKey}
                    mine={row.message.authorId === meId}
                    animate={!initial.current?.has(row.message.id)}
                    canDelete={manager}
                    canWave={canSend}
                    actions={actions}
                    clock={clock}
                  />
                );
              const from = foreignServer(row.message, serverId);
              return (
                <MessageRow
                  key={row.key}
                  from={from}
                  canKeepOut={keepsOut && !!from}
                  message={row.message}
                  first={row.first}
                  display={display}
                  developer={developer}
                  date={row.date}
                  author={author}
                  member={memberById.get(row.message.authorId)}
                  mine={row.message.authorId === meId}
                  mentionsMe={pingsUser(me, myRoleIds ?? EMPTY, row.message, suppressEveryone)}
                  instanceKey={instanceKey}
                  canDelete={row.message.authorId === meId || (manager && !(guestSide && from))}
                  animate={!initial.current?.has(row.message.id)}
                  editing={editing === row.message.id}
                  canThread={!row.message.threadId && (row.message.thread ? canReply : canStart)}
                  inThread={!!threadId}
                  actions={actions}
                  clock={clock}
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
      <SharedNote channel={channel} />
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
export function AuthorName({ user, member, app = false }: { user: User | undefined; member: Member | undefined; app?: boolean }) {
  const { ownerId } = useServerLook();
  const color = useRoleColor(member);
  return (
    <span className="inline-flex min-w-0 items-center gap-1">
      <RoleName id={user?.id ?? ""} name={member?.nickname || displayName(user)} color={color} />
      {!!ownerId && user?.id === ownerId && <CrownIcon aria-label="Owner" className="size-3.5 shrink-0 text-amber-400" />}
      {(app || isAgent(user)) && <AppBadge agent={!app} />}
    </span>
  );
}

/** Who a webhook message says it's from, as a profile to draw. */
export const webhookAuthor = (w: MessageWebhook): User =>
  ({ id: w.webhookId, displayName: w.name, username: w.name, avatarUrl: w.avatarUrl }) as User;

/** What makes a run of messages one author's: a webhook posting under another name starts a new one. */
const runKey = (m: Message) => (m.webhook ? `${m.authorId}\n${m.webhook.name}\n${m.webhook.avatarUrl}` : m.authorId);

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
  app = false,
  from,
  children,
}: {
  display: MessageDisplay;
  first: boolean;
  author: User | undefined;
  member: Member | undefined;
  /** In a shared channel, the server the author is from when it isn't this one. */
  from?: SharedServer | null;
  /** Posted by an app through a webhook: marked, with no profile to open. */
  app?: boolean;
  /** When it was sent; missing while it's still sending. */
  date?: Date;
  /** Shown instead of the time, like "sending…". */
  status?: string;
  /** Where the author's profile card loads from; without it, names don't open one. */
  instanceKey?: string;
  children: React.ReactNode;
}) {
  const card = (child: React.ReactElement) =>
    instanceKey && author && !app ? (
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
              <AuthorName user={author} member={member} app={app} />
            </button>,
          )}
        </span>
        {from && <ServerTag server={from} className="mr-1.5" />}
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
                <AuthorName user={author} member={member} app={app} />
              </button>,
            )}
            {from && <ServerTag server={from} className="self-center" />}
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
export function MessageBody({
  content,
  emojis,
  display,
  className,
}: {
  content: string;
  /** Emoji from other servers the message brought along. */
  emojis?: Emoji[];
  display: MessageDisplay;
  className?: string;
}) {
  return (
    <MessageEmojis value={emojis}>
      <Markdown className={cn("chat", display === "compact" && "inline-first", onlyEmoji(content) && "jumbo", className)} extension={CHAT}>
        {content}
      </Markdown>
    </MessageEmojis>
  );
}

/** A row's `clock` is only there so a new clock setting redraws its times. */
type Redraw = { clock: Clock };

const MessageRow = memo(function MessageRow({
  message,
  from,
  canKeepOut,
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
  canThread,
  inThread,
  actions,
}: Redraw & {
  message: Message;
  from: SharedServer | null;
  /** At a shared channel's home, with Kick Members: this author is from another server and can be kept out. */
  canKeepOut: boolean;
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
  /** Can open the thread under it: reply in the one there, or start one. */
  canThread: boolean;
  /** Drawn in a thread's own list, where replies don't get threads of their own. */
  inThread: boolean;
  actions: RowActions;
}) {
  const [confirming, setConfirming] = useState<"delete" | "keep-out" | false>(false);
  const [copied, setCopied] = useState(false);
  const edited = !!message.editedAt;
  return (
    <motion.div
      {...(animate ? enter : {})}
      exit={{ opacity: 0, height: 0, transition: { duration: 0.2 } }}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      data-message-id={message.id}
      className={cn(
        "message-row group relative flex gap-3 px-4",
        first && "first",
        display === "compact" && "compact",
        mentionsMe && "mention-me",
        animate && mine && "landed",
      )}
    >
      <MessageLine display={display} first={first} author={author} member={member} date={date} instanceKey={instanceKey} app={!!message.webhook} from={from}>
        {editing ? (
          <EditBox initial={message.content.replace(EMOJI_TOKEN, ":$2:")} onCancel={actions.cancelEdit} onSave={(content) => actions.save(message.id, content)} />
        ) : (
          <>
            {message.threadId && <AlsoSentNote message={message} inThread={inThread} onOpen={actions.thread} />}
            {message.content && <MessageBody content={message.content} emojis={message.emojis} display={display} />}
            {edited && (
              <span className="text-[0.7rem] text-muted-foreground" title={formatFull(toDate(message.editedAt))}>
                {" "}
                (edited)
              </span>
            )}
            <Attachments files={message.attachments} animate={animate} />
            <Embeds embeds={message.embeds} animate={animate} />
            {!inThread && message.thread && <RepliesRow instanceKey={instanceKey} message={message} onOpen={actions.thread} />}
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
              <span className="px-2 text-xs font-bold text-destructive">{confirming === "keep-out" ? `Keep ${displayName(author)} out?` : "Delete?"}</span>
              <ToolButton
                label={confirming === "keep-out" ? "Keep out of this channel" : "Delete"}
                danger
                onClick={() =>
                  (confirming === "keep-out" ? actions.keepOut(message.authorId, displayName(author)) : actions.remove(message.id)).catch((err: Error) => {
                    if (confirming === "keep-out") toast(err.message);
                    setConfirming(false);
                  })
                }
              >
                <CheckIcon />
              </ToolButton>
              <ToolButton label="Cancel" onClick={() => setConfirming(false)}>
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
              {canThread && (
                <ToolButton label={message.thread ? "Open thread" : "Reply in thread"} onClick={() => actions.thread(message.id)}>
                  <MessageSquareReplyIcon />
                </ToolButton>
              )}
              {developer && (
                <ToolButton label="Copy message ID" onClick={() => copy(message.id, "message ID")}>
                  <FingerprintIcon />
                </ToolButton>
              )}
              {mine && (
                <ToolButton label="Edit" onClick={() => actions.edit(message.id)}>
                  <PencilIcon />
                </ToolButton>
              )}
              {canKeepOut && (
                <ToolButton label="Keep out of this channel" danger onClick={() => setConfirming("keep-out")}>
                  <UserXIcon />
                </ToolButton>
              )}
              {canDelete && (
                <ToolButton label="Delete" danger onClick={() => setConfirming("delete")}>
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

/** The words in `text` that set a rule off, marked. */
function marked(text: string, matched: string[]): ReactNode {
  const words = matched.filter((m) => m && !/^\d+ pings$/.test(m)).sort((a, b) => b.length - a.length);
  if (!words.length) return text;
  const escaped = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  return text.split(new RegExp(`(${escaped.join("|")})`, "gi")).map((part, n) =>
    n % 2 ? (
      <mark key={n} className="rounded bg-destructive/20 px-0.5 text-destructive">
        {part}
      </mark>
    ) : (
      part
    ),
  );
}

const TRIGGER_LABEL: Record<number, string> = {
  [AutoModTrigger.KEYWORDS]: "blocked words",
  [AutoModTrigger.MENTION_SPAM]: "mention spam",
  [AutoModTrigger.LINKS]: "a link",
};

/** What AutoMod caught, for the mods in the alert channel: who, where, what it said and what was done. */
const AutoModAlertRow = memo(function AutoModAlertRow({
  message,
  date,
  author,
  member,
  instanceKey,
  channelName,
  animate,
  canDelete,
  actions,
}: Redraw & {
  message: Message;
  date: Date;
  author: User | undefined;
  member: Member | undefined;
  instanceKey: string;
  channelName: string | undefined;
  animate: boolean;
  canDelete: boolean;
  actions: RowActions;
}) {
  const alert = message.autoMod;
  const color = useRoleColor(member);
  const [confirming, setConfirming] = useState(false);
  if (!alert) return null;
  return (
    <motion.div
      {...(animate ? enter : {})}
      exit={{ opacity: 0, height: 0, transition: { duration: 0.2 } }}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      className="message-row group relative flex gap-3 px-4 py-1.5"
    >
      <span className="w-10 shrink-0 pt-1">
        <motion.span
          initial={animate ? { scale: 0, rotate: -30 } : false}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.1 }}
          className="grid size-10 place-items-center rounded-full bg-gradient-to-br from-amber-400 to-rose-500 text-white shadow-md"
        >
          <ShieldAlertIcon className="size-5" />
        </motion.span>
      </span>
      <div className="min-w-0 flex-1">
        <p className="flex flex-wrap items-baseline gap-x-2 text-sm">
          <span className="font-extrabold">AutoMod</span>
          <span className="rounded bg-primary/15 px-1 text-[0.6rem] font-extrabold text-primary uppercase">Bot</span>
          <time className="text-xs text-muted-foreground" dateTime={date.toISOString()} title={formatFull(date)}>
            {formatStamp(date)}
          </time>
        </p>
        {alert.cappedPerDay > 0 ? (
          <div className="mt-1 overflow-hidden rounded-2xl border border-l-4 border-l-amber-500 bg-card/70 p-3">
            <p className="text-sm">
              The Smart filter used up today's <b>{alert.cappedPerDay.toLocaleString()}</b> checks, so messages go
              through it unchecked until midnight UTC. Your other rules still apply.
            </p>
            <div className="mt-2 flex flex-wrap items-center gap-1.5 text-xs">
              <span className="flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 font-bold">
                <ShieldIcon className="size-3" /> {alert.ruleName}
              </span>
              <span className="flex items-center gap-1 rounded-full bg-amber-500/15 px-2 py-0.5 font-bold text-amber-600 dark:text-amber-400">
                <TimerIcon className="size-3" /> Back at midnight UTC
              </span>
            </div>
          </div>
        ) : (
        <div className="mt-1 overflow-hidden rounded-2xl border border-l-4 border-l-amber-500 bg-card/70 p-3">
          <p className="text-sm">
            {alert.blocked ? "Blocked a message from " : "Flagged a message from "}
            <ProfilePopover instanceKey={instanceKey} user={author} member={member}>
              <button type="button" className="inline-flex align-bottom font-bold hover:underline">
                <RoleName id={message.authorId} name={member?.nickname || displayName(author)} color={color} />
              </button>
            </ProfilePopover>
            {channelName && (
              <>
                {" "}
                in <b>#{channelName}</b>
              </>
            )}{" "}
            for {TRIGGER_LABEL[alert.trigger] ?? "breaking a rule"}.
          </p>
          <blockquote className="mt-2 max-h-40 overflow-y-auto rounded-xl bg-muted/60 px-3 py-2 text-sm break-words whitespace-pre-wrap text-muted-foreground">
            {marked(alert.content, alert.matched)}
          </blockquote>
          <div className="mt-2 flex flex-wrap items-center gap-1.5 text-xs">
            <span className="flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 font-bold">
              <ShieldIcon className="size-3" /> {alert.ruleName}
            </span>
            {alert.matched.map((m) => (
              <code key={m} className="rounded-md bg-destructive/10 px-1.5 py-0.5 text-destructive">
                {m}
              </code>
            ))}
            {alert.timedOutSeconds > 0 && (
              <span className="flex items-center gap-1 rounded-full bg-amber-500/15 px-2 py-0.5 font-bold text-amber-600 dark:text-amber-400">
                <TimerIcon className="size-3" /> Timed out for {formatDuration(alert.timedOutSeconds)}
              </span>
            )}
          </div>
        </div>
        )}
      </div>
      {canDelete && (
        <div className="message-tools absolute -top-3 right-4 z-10 flex items-center gap-0.5 rounded-xl border bg-card p-0.5 shadow-md">
          {confirming ? (
            <span className="flex items-center gap-0.5">
              <span className="px-2 text-xs font-bold text-destructive">Delete?</span>
              <ToolButton label="Delete" danger onClick={() => actions.remove(message.id).catch(() => setConfirming(false))}>
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
});

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
const JoinRow = memo(function JoinRow({
  message,
  date,
  author,
  member,
  instanceKey,
  mine,
  animate,
  canDelete,
  canWave,
  actions,
}: Redraw & {
  message: Message;
  date: Date;
  author: User | undefined;
  member: Member | undefined;
  instanceKey: string;
  mine: boolean;
  animate: boolean;
  canDelete: boolean;
  /** False where you can't send messages, such as before agreeing to the rules. */
  canWave: boolean;
  actions: RowActions;
}) {
  const [done, setDone] = useState(() => waved.has(message.id));
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
      {!mine && author && canWave && (
        <motion.button
          type="button"
          disabled={done || waving}
          whileTap={{ scale: 0.9 }}
          onClick={async () => {
            setWaving(true);
            try {
              await actions.wave(author.username);
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
              <ToolButton label="Delete" danger onClick={() => actions.remove(message.id).catch(() => setConfirming(false))}>
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
});

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

const PendingRow = memo(function PendingRow({
  pending,
  first,
  display,
  me,
  member,
  actions,
}: {
  pending: PendingMessage;
  first: boolean;
  display: MessageDisplay;
  me: User | undefined;
  member: Member | undefined;
  actions: RowActions;
}) {
  const onRetry = () => actions.retry(pending);
  const onDismiss = () => actions.dismiss(pending.nonce);
  // A message AutoMod stopped: why, without a retry that would only be stopped again.
  const blocked = pending.failed?.startsWith("AutoMod: ") ? pending.failed.slice("AutoMod: ".length) : null;
  return (
    <motion.div
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: pending.failed ? 1 : 0.55, y: 0 }}
      exit={{ opacity: 0 }}
      transition={{ type: "spring", stiffness: 500, damping: 34 }}
      className={cn("message-row flex gap-3 px-4", first && "first", display === "compact" && "compact")}
    >
      <MessageLine display={display} first={first} author={me} member={member} status="sending…">
        {pending.content && (
          <MessageBody content={pending.content} display={display} className={cn(pending.failed && "text-destructive", blocked && "line-through decoration-destructive/50")} />
        )}
        <PendingFiles files={pending.files ?? EMPTY} />
        {blocked ? (
          <motion.div
            initial={{ opacity: 0, y: -4, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            transition={{ type: "spring", stiffness: 500, damping: 26 }}
            className="mt-1.5 flex flex-wrap items-center gap-2 rounded-xl border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs"
          >
            <motion.span animate={{ rotate: [0, -12, 12, -6, 0] }} transition={{ duration: 0.6, delay: 0.15 }}>
              <ShieldAlertIcon className="size-4 text-amber-600 dark:text-amber-400" />
            </motion.span>
            <span className="min-w-0 flex-1">
              <b>AutoMod didn't send this.</b> <span className="text-muted-foreground first-letter:uppercase">{blocked}</span>
            </span>
            <button
              type="button"
              onClick={() => void navigator.clipboard?.writeText(pending.content)}
              className="inline-flex items-center gap-1 font-bold text-primary hover:underline"
            >
              <CopyIcon className="size-3" /> Copy text
            </button>
            <button type="button" onClick={onDismiss} className="font-bold text-muted-foreground hover:underline">
              Dismiss
            </button>
          </motion.div>
        ) : pending.failed && (
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
});
