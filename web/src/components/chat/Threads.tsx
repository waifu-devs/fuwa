import {
  ArchiveIcon,
  BellIcon,
  BellOffIcon,
  CornerDownRightIcon,
  CornerUpLeftIcon,
  LockIcon,
  LockOpenIcon,
  MessagesSquareIcon,
  SearchIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { ListThreadsResponse } from "@/gen/fuwa/v1/message_pb";
import { Permission, type Channel, type Message, type ThreadSummary, type User } from "@/gen/fuwa/v1/types_pb";
import { focusThread, followThread, listThreads, loadFollowed, lockThread, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { Composer } from "@/components/chat/Composer";
import { Embeds } from "@/components/chat/Embeds";
import { useServerLook } from "@/components/chat/mentions";
import { MessageBody, MessageLine, MessageList, type MessageListHandle } from "@/components/chat/MessageList";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { ago, displayName, toDate } from "@/lib/format";
import { type Key, T, useI18n } from "@/i18n/react";
import { hasIn } from "@/lib/permissions";
import { isArchived, type ThreadPanelState } from "@/lib/threads";
import { usePrefs } from "@/lib/prefs";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/*
 * Threads: replies under a message, beside the channel (Slack's way). The
 * channel shows a row under each message with a thread; the thread itself
 * opens in a panel with its own list and composer. Replies are ordinary
 * messages with a thread id, kept in the store under `threadKey`.
 */

const EMPTY: never[] = [];

/** One of the replies row's two labels, stacked in the same grid cell: the last reply, or "View thread" while hovered. */
export const SWAP = "truncate transition duration-200 ease-out [grid-area:1/1]";

const useArchiveHours = (instanceKey: string, serverId: string) =>
  useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.threadArchiveHours ?? 0);

/** Up to three faces, overlapping, popping in as people join. */
export function Faces({ instanceKey, ids, size = "size-5" }: { instanceKey: string; ids: string[]; size?: string }) {
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  return (
    <span className="flex -space-x-1.5">
      <AnimatePresence initial={false}>
        {ids.slice(0, 3).map((id, n) => (
          <motion.span
            key={id}
            initial={{ scale: 0, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            exit={{ scale: 0, opacity: 0 }}
            transition={{ ...SPRING, delay: n * 0.04 }}
            style={{ zIndex: 3 - n }}
            className="rounded-full ring-2 ring-background"
          >
            <UserAvatar user={users?.[id]} className={cn(size, "text-[0.55rem]")} />
          </motion.span>
        ))}
      </AnimatePresence>
    </span>
  );
}

/**
 * Under a message with a thread: who replied, how many, and when last. The
 * count rolls when replies come in; a dot marks new ones in a thread you follow.
 */
export const RepliesRow = memo(function RepliesRow({
  instanceKey,
  message,
  onOpen,
}: {
  instanceKey: string;
  message: Message;
  onOpen: (threadId: string) => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const thread = message.thread;
  const unread = useFuwa((s) => s.instances[instanceKey]?.threadUnread[message.id] ?? 0);
  const hours = useArchiveHours(instanceKey, message.serverId);
  if (!thread || (thread.replyCount === 0 && !thread.locked)) return null;
  const archived = isArchived(thread, hours);
  const last = toDate(thread.lastReplyAt);
  return (
    <motion.button
      type="button"
      onClick={() => onOpen(message.id)}
      initial={{ opacity: 0, y: -4 }}
      animate={{ opacity: 1, y: 0 }}
      transition={SPRING}
      whileTap={{ scale: 0.98 }}
      className="group/replies mt-1 -ml-1.5 flex max-w-full items-center gap-2 rounded-xl border border-transparent px-1.5 py-1 text-left text-xs transition-colors hover:border-border hover:bg-card/70"
    >
      <Faces instanceKey={instanceKey} ids={thread.participantIds} />
      <span className="flex shrink-0 items-center gap-1 font-bold text-primary">
        <T k="chat.threads.replies" values={{ count: <Count value={thread.replyCount} /> }} count={thread.replyCount} />
      </span>
      <AnimatePresence initial={false}>
        {unread > 0 && (
          <motion.span
            initial={{ scale: 0 }}
            animate={{ scale: 1 }}
            exit={{ scale: 0 }}
            transition={SPRING}
            className="rounded-full bg-primary px-1.5 text-[0.65rem] font-extrabold text-primary-foreground tabular-nums"
          >
            {unread > 99 ? t("chat.threads.newMany") : t("chat.threads.newCount", { count: unread })}
          </motion.span>
        )}
      </AnimatePresence>
      {thread.locked && <LockIcon aria-label={t("chat.threads.locked")} className="size-3 shrink-0 text-muted-foreground" />}
      {/* Both labels share one grid cell, so the row keeps the wider one's width and never shrinks out from under the pointer. */}
      <span className="grid min-w-0 text-muted-foreground">
        <span className={cn(SWAP, "group-hover/replies:-translate-y-1 group-hover/replies:opacity-0 group-focus-visible/replies:-translate-y-1 group-focus-visible/replies:opacity-0")}>
          {archived ? t("chat.threads.archived") : t("chat.threads.lastReply", { time: ago(lang, last) })}
        </span>
        <span aria-hidden className={cn(SWAP, "translate-y-1 opacity-0 group-hover/replies:translate-y-0 group-hover/replies:opacity-100 group-focus-visible/replies:translate-y-0 group-focus-visible/replies:opacity-100")}>
          {t("chat.threads.view")}
        </span>
      </span>
    </motion.button>
  );
});

/** On a thread reply: in the channel, that it came from a thread; in the thread, that it was also sent to the channel. */
export function AlsoSentNote({ message, inThread, onOpen }: { message: Message; inThread: boolean; onOpen: (threadId: string) => void }) {
  const { t } = useI18n();
  if (!message.alsoInChannel) return null;
  if (inThread) return <p className="text-[0.7rem] font-bold text-muted-foreground">{t("chat.threads.alsoSent")}</p>;
  return (
    <button
      type="button"
      onClick={() => onOpen(message.threadId)}
      className="flex items-center gap-1 text-[0.7rem] font-bold text-muted-foreground transition-colors hover:text-primary"
    >
      <CornerDownRightIcon className="size-3" /> {t("chat.threads.repliedInThread")}
    </button>
  );
}

/** The top of a thread: the message it's under, then how many replies follow. */
function ThreadStart({ instanceKey, parent }: { instanceKey: string; parent: Message | undefined }) {
  const { t } = useI18n();
  const display = usePrefs((p) => p.messageDisplay);
  const look = useServerLook();
  const author = useFuwa((s) => (parent ? s.instances[instanceKey]?.users[parent.authorId] : undefined));
  const member = look.members.find((m) => m.user?.id === parent?.authorId);
  if (!parent) return <div className="shimmer mx-4 my-4 h-16 rounded-2xl" />;
  const count = parent.thread?.replyCount ?? 0;
  return (
    <div className="pt-3">
      <div className="message-row first flex gap-3 px-4">
        <MessageLine
          display={display}
          first
          author={
            parent.webhook
              ? ({
                  id: parent.authorId,
                  displayName: parent.webhook.name,
                  username: parent.webhook.name,
                  avatarUrl: parent.webhook.avatarUrl,
                } as never)
              : author
          }
          member={member}
          date={toDate(parent.createdAt)}
          instanceKey={instanceKey}
          app={!!parent.webhook}
        >
          {parent.content && <MessageBody content={parent.content} display={display} />}
          <Embeds embeds={parent.embeds} animate={false} />
        </MessageLine>
      </div>
      <div role="separator" className="my-3 flex items-center gap-3 px-4 text-xs font-bold text-muted-foreground">
        <span>
          {count === 0 ? t("chat.threads.noReplies") : <T k="chat.threads.replies" values={{ count: <Count value={count} /> }} count={count} />}
        </span>
        <span className="h-px flex-1 bg-border" />
      </div>
    </div>
  );
}

/** The header button that opens a channel's threads. */
export function ThreadsButton({ open, active, onClick }: { open: boolean; active: boolean; onClick: () => void }) {
  const { t } = useI18n();
  return (
    <motion.button
      type="button"
      aria-label={open ? t("chat.threads.hide") : t("chat.threads.threads")}
      title={t("chat.threads.threads")}
      aria-pressed={open}
      onClick={onClick}
      whileTap={{ scale: 0.85 }}
      className={cn(
        "grid size-9 place-items-center rounded-full transition-colors hover:bg-muted",
        active ? "bg-primary/10 text-primary" : "text-muted-foreground",
      )}
    >
      <motion.span initial={false} animate={{ rotate: active ? 0 : -8, scale: active ? 1.08 : 1 }} transition={SPRING}>
        <MessagesSquareIcon className="size-5" />
      </motion.span>
    </motion.button>
  );
}

export function PanelButton({
  label,
  onClick,
  active = false,
  children,
}: {
  label: string;
  onClick: () => void;
  active?: boolean;
  children: ReactNode;
}) {
  return (
    <motion.button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={active}
      onClick={onClick}
      whileTap={{ scale: 0.85 }}
      className={cn(
        "grid size-8 place-items-center rounded-full transition-colors hover:bg-muted [&_svg]:size-4",
        active ? "bg-primary/10 text-primary" : "text-muted-foreground",
      )}
    >
      {children}
    </motion.button>
  );
}

/** The top of a channel's list of threads: what it is, where, and a way to close it. */
export function ThreadListHeader({ where, onClose }: { where: string; onClose: () => void }) {
  const { t } = useI18n();
  return (
    <header className="flex h-14 shrink-0 items-center gap-2 border-b px-3">
      <MessagesSquareIcon className="size-5 shrink-0 text-primary" />
      <div className="min-w-0 flex-1">
        <h2 className="truncate leading-tight font-extrabold">{t("chat.threads.threads")}</h2>
        <p className="truncate text-xs text-muted-foreground">{where}</p>
      </div>
      <PanelButton label={t("chat.threads.closeList")} onClick={onClose}>
        <XIcon />
      </PanelButton>
    </header>
  );
}

// Where a thread's header says it is: its channel, and whether it's archived or locked.
function whereKey(archived: boolean, locked: boolean): Key {
  if (archived && locked) return "chat.threads.whereArchivedLocked";
  if (archived) return "chat.threads.whereArchived";
  return locked ? "chat.threads.whereLocked" : "chat.threads.where";
}

/**
 * A thread beside its channel: the message it's under, its replies (drawn
 * a window at a time, as the channel is) and a composer of its own, with
 * "Also send to #channel". Follow, lock (for moderators) and jump to the
 * message it's under sit in its header.
 */
export function ThreadPanel({
  instanceKey,
  serverId,
  channel,
  threadId,
  onClose,
  onJump,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  threadId: string;
  onClose: () => void;
  onJump: (id: string) => void;
}) {
  const { t } = useI18n();
  const parent = useFuwa(
    (s) =>
      s.instances[instanceKey]?.threadParents[threadId] ??
      s.instances[instanceKey]?.messages[channel.id]?.items.find((m) => m.id === threadId),
  );
  const followed = useFuwa((s) => !!s.instances[instanceKey]?.followed[serverId]?.[threadId]);
  const access = useAccess(instanceKey, serverId);
  // Only the channel's home locks its threads.
  const manager = hasIn(access, channel.id, Permission.MANAGE_MESSAGES) && !(channel.shared && !channel.shared.home);
  const list = useRef<MessageListHandle>(null);
  const locked = !!parent?.thread?.locked;
  const archived = isArchived(parent?.thread, useArchiveHours(instanceKey, serverId));

  useEffect(() => {
    focusThread(instanceKey, threadId);
    return () => focusThread(instanceKey, null);
  }, [instanceKey, threadId]);
  useEffect(() => {
    run(loadFollowed(instanceKey, serverId)).catch(() => {});
  }, [instanceKey, serverId]);

  const fail = (err: FuwaError) => toast(err.message);
  const follow = () =>
    run(followThread(instanceKey, serverId, channel.id, threadId, !followed))
      .then(() => toast(followed ? t("chat.threads.unfollowed") : t("chat.threads.followed")))
      .catch(fail);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-14 shrink-0 items-center gap-2 border-b px-3">
        <MessagesSquareIcon className="size-5 shrink-0 text-primary" />
        <div className="min-w-0 flex-1">
          <h2 className="truncate leading-tight font-extrabold">{t("chat.threads.thread")}</h2>
          <p className="truncate text-xs text-muted-foreground">
            {t(whereKey(archived, locked), { channel: channel.name })}
          </p>
        </div>
        {!!parent?.thread && (
          <PanelButton label={followed ? t("chat.threads.unfollow") : t("chat.threads.follow")} active={followed} onClick={follow}>
            <motion.span
              key={followed ? "on" : "off"}
              initial={{ rotate: -25, scale: 0.6, opacity: 0 }}
              animate={{ rotate: 0, scale: 1, opacity: 1 }}
              transition={SPRING}
              className="grid place-items-center"
            >
              {followed ? <BellIcon /> : <BellOffIcon />}
            </motion.span>
          </PanelButton>
        )}
        {manager && !!parent?.thread && (
          <PanelButton
            label={locked ? t("chat.threads.unlock") : t("chat.threads.lock")}
            active={locked}
            onClick={() => run(lockThread(instanceKey, serverId, channel.id, threadId, !locked)).catch(fail)}
          >
            {locked ? <LockIcon /> : <LockOpenIcon />}
          </PanelButton>
        )}
        <PanelButton label={t("chat.threads.jump")} onClick={() => onJump(threadId)}>
          <CornerUpLeftIcon />
        </PanelButton>
        <PanelButton label={t("chat.threads.close")} onClick={onClose}>
          <XIcon />
        </PanelButton>
      </header>
      <MessageList
        key={threadId}
        ref={list}
        instanceKey={instanceKey}
        serverId={serverId}
        channel={channel}
        threadId={threadId}
        header={<ThreadStart instanceKey={instanceKey} parent={parent} />}
      />
      <Composer
        instanceKey={instanceKey}
        serverId={serverId}
        channel={channel}
        thread={{ id: threadId, locked: locked && !manager, started: !!parent?.thread }}
        placeholder={t("chat.threads.placeholder")}
        onEditLast={() => list.current?.editLast()}
      />
    </div>
  );
}

// Previews show text, not Markdown marks.
function plain(content: string) {
  return content
    .replace(/[*_~`>#]+/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** The thread list's search box, and Open and Archived when threads get archived. */
export function ThreadFilters({
  query,
  onQuery,
  tabs,
  archived,
  onArchived,
  glide,
}: {
  query: string;
  onQuery: (query: string) => void;
  tabs: boolean[];
  archived: boolean;
  onArchived: (archived: boolean) => void;
  /** The selected tab's layout id, one per list. */
  glide: string;
}) {
  const { t } = useI18n();
  return (
    <>
      <label className="flex items-center gap-2 rounded-xl border bg-card px-2.5 py-1.5 focus-within:border-primary/50">
        <SearchIcon className="size-4 shrink-0 text-muted-foreground" />
        <input
          value={query}
          onChange={(e) => onQuery(e.target.value.slice(0, 100))}
          placeholder={t("chat.threads.search")}
          aria-label={t("chat.threads.search")}
          className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
        />
      </label>
      {tabs.length > 1 && (
        <div className="relative grid grid-cols-2 rounded-xl bg-muted p-0.5 text-xs font-bold">
          {tabs.map((tab) => (
            <button
              key={String(tab)}
              type="button"
              onClick={() => onArchived(tab)}
              className={cn(
                "relative z-10 flex items-center justify-center gap-1 rounded-lg py-1.5 transition-colors",
                archived === tab ? "text-foreground" : "text-muted-foreground",
              )}
            >
              {archived === tab && (
                <motion.span
                  layoutId={glide}
                  transition={SPRING}
                  className="absolute inset-0 -z-10 rounded-lg bg-card shadow-sm"
                />
              )}
              {tab ? <ArchiveIcon className="size-3.5" /> : <MessagesSquareIcon className="size-3.5" />}
              {tab ? t("chat.threads.archived") : t("chat.threads.open")}
            </button>
          ))}
        </div>
      )}
    </>
  );
}

/**
 * A channel's threads, the latest reply first: open ones, or archived ones,
 * searched by what their message or replies say.
 */
export function ThreadList({
  instanceKey,
  serverId,
  channel,
  onOpen,
  onClose,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  onOpen: (threadId: string) => void;
  onClose: () => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const [query, setQuery] = useState("");
  const [archived, setArchived] = useState(false);
  const [page, setPage] = useState<ListThreadsResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const hours = useArchiveHours(instanceKey, serverId);
  const users = useFuwa((s) => s.instances[instanceKey]?.users);

  useEffect(() => {
    let live = true;
    setLoading(true);
    const timer = setTimeout(
      () =>
        run(listThreads(instanceKey, serverId, channel.id, { query: query.trim(), archived }))
          .then((res) => live && (setPage(res), setError(null)))
          .catch((err: FuwaError) => live && setError(err.message))
          .finally(() => live && setLoading(false)),
      query ? 250 : 0,
    );
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [instanceKey, serverId, channel.id, query, archived]);

  const more = () => {
    const after = page?.nextAfterThreadId || page?.threads.at(-1)?.id;
    if (!after) return;
    setLoading(true);
    run(listThreads(instanceKey, serverId, channel.id, { query: query.trim(), archived, afterThreadId: after }))
      .then((res) => setPage((p) => (p ? { ...res, threads: [...p.threads, ...res.threads] } : res)))
      .catch((err: FuwaError) => toast(err.message))
      .finally(() => setLoading(false));
  };

  const threads = page?.threads ?? EMPTY;
  const tabs = useMemo(() => (hours > 0 ? [false, true] : [false]), [hours]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <ThreadListHeader where={t("chat.threads.where", { channel: channel.name })} onClose={onClose} />
      <div className="flex flex-col gap-2 border-b p-3">
        <ThreadFilters query={query} onQuery={setQuery} tabs={tabs} archived={archived} onArchived={setArchived} glide={`thread-tab-${channel.id}`} />
      </div>
      <div className="scroll-thin min-h-0 flex-1 overflow-y-auto p-2">
        {error && <p className="p-3 text-sm text-destructive">{error}</p>}
        {!error && !loading && threads.length === 0 && !page?.hasMore && <NoThreads query={query} archived={archived} hours={hours} />}
        <AnimatePresence initial={false}>
          {threads.map((m, n) => (
            <ThreadRow key={m.id} instanceKey={instanceKey} message={m} index={n} users={users} onOpen={onOpen} />
          ))}
        </AnimatePresence>
        {loading && <div className="shimmer m-2 h-16 rounded-xl" />}
        {!loading && page?.hasMore && (
          <button
            type="button"
            onClick={more}
            className="mx-auto my-2 block rounded-full px-3 py-1 text-xs font-bold text-primary hover:bg-primary/10"
          >
            {threads.length === 0 ? t("chat.threads.searchOlder") : t("chat.threads.showMore")}
          </button>
        )}
      </div>
    </div>
  );
}

/** Nothing to list: no threads yet, none archived, or none matching the search. */
function NoThreads({ query, archived, hours }: { query: string; archived: boolean; hours: number }) {
  const { t } = useI18n();
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={SPRING}
      className="grid place-items-center gap-2 px-6 py-12 text-center"
    >
      <span className="float grid size-12 place-items-center rounded-full bg-primary/10 text-primary">
        <MessagesSquareIcon className="size-6" />
      </span>
      <p className="text-sm font-bold">{query ? t("chat.threads.noMatches") : archived ? t("chat.threads.noArchived") : t("chat.threads.none")}</p>
      <p className="text-xs text-muted-foreground">
        {query
          ? t("chat.threads.tryOtherWords")
          : archived
            ? hours >= 48
              ? t("chat.threads.archivedHintDays", { count: Math.round(hours / 24) })
              : t("chat.threads.archivedHintHours", { count: hours })
            : t("chat.threads.noneHint")}
      </p>
    </motion.div>
  );
}

/** One thread in the list: who started it, what it says, and how its replies are going. */
function ThreadRow({
  instanceKey,
  message,
  index,
  users,
  onOpen,
}: {
  instanceKey: string;
  message: Message;
  index: number;
  users: Record<string, User> | undefined;
  onOpen: (threadId: string) => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  return (
    <motion.button
      type="button"
      layout="position"
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0 }}
      transition={{ ...SPRING, delay: Math.min(index, 8) * 0.025 }}
      onClick={() => onOpen(message.id)}
      className="flex w-full gap-2.5 rounded-xl p-2.5 text-left transition-colors hover:bg-muted/70"
    >
      <UserAvatar user={users?.[message.authorId]} className="size-8 text-xs" />
      <span className="min-w-0 flex-1">
        <span className="flex items-baseline gap-2">
          <b className="truncate text-sm">{message.webhook?.name || displayName(users?.[message.authorId])}</b>
          <span className="shrink-0 text-[0.7rem] text-muted-foreground">{ago(lang, toDate(message.createdAt))}</span>
        </span>
        <span className="line-clamp-2 text-sm break-words text-muted-foreground">{plain(message.content) || message.embeds[0]?.title || "…"}</span>
        <span className="mt-1 flex items-center gap-2 text-xs">
          <Faces instanceKey={instanceKey} ids={message.thread?.participantIds ?? EMPTY} size="size-4" />
          <b className="text-primary">{t("chat.threads.replies", { count: message.thread?.replyCount ?? 0 })}</b>
          {message.thread?.locked && <LockIcon className="size-3 text-muted-foreground" />}
          <span className="truncate text-muted-foreground">{t("chat.threads.lastAgo", { time: ago(lang, toDate(message.thread?.lastReplyAt)) })}</span>
        </span>
      </span>
    </motion.button>
  );
}

/** What sits beside the channel: one open thread, or the channel's list of them. */
export function ThreadSide({
  instanceKey,
  serverId,
  channel,
  panel,
  onOpen,
  onClose,
  onJump,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  panel: NonNullable<ThreadPanelState>;
  onOpen: (threadId: string) => void;
  onClose: () => void;
  onJump: (id: string) => void;
}) {
  if (panel.kind === "thread")
    return (
      <ThreadPanel
        key={panel.id}
        instanceKey={instanceKey}
        serverId={serverId}
        channel={channel}
        threadId={panel.id}
        onClose={onClose}
        onJump={onJump}
      />
    );
  return <ThreadList instanceKey={instanceKey} serverId={serverId} channel={channel} onOpen={onOpen} onClose={onClose} />;
}
