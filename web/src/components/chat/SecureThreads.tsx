import { ArchiveIcon, BellIcon, BellOffIcon, CornerDownRightIcon, LockIcon, LockOpenIcon, MessagesSquareIcon, SearchIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import type { Channel, Member, User } from "@/gen/fuwa/v1/types_pb";
import { archived, following, search, unreadIn, type Organized, type SecureThread } from "@/e2ee/threads";
import { lineText, type Item } from "@/e2ee/vault";
import { dmProblem, followSecureThread, lockSecureThread, markSecureThreadRead } from "@/fuwa/dms";
import { useFuwa, type PendingMessage, type ThreadNote } from "@/fuwa/store";
import { EncryptedComposer, EncryptedMessages, type ThreadHooks } from "@/components/dm/DmView";
import { Faces, PanelButton, SWAP, ThreadFilters, ThreadListHeader } from "@/components/chat/Threads";
import { MessageBody, MessageLine } from "@/components/chat/MessageList";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { ago, displayName } from "@/lib/format";
import { T, useI18n } from "@/i18n/react";
import { usePrefs } from "@/lib/prefs";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/*
 * Threads inside a secure channel. The same panel and list as an ordinary
 * channel's (Threads.tsx), fed from what this device opened instead of the
 * server's summaries: the server can't read the channel, so it can't tell
 * which lines are replies. Nothing here asks it for anything.
 */

const NO_ITEMS: Item[] = [];
const NO_NOTE: ThreadNote = { follows: {}, read: {} };

export const useArchiveHours = (instanceKey: string, serverId: string) =>
  useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.threadArchiveHours ?? 0);

export const useThreadNote = (instanceKey: string, id: string) => useFuwa((s) => s.instances[instanceKey]?.dms.threadNotes[id] ?? NO_NOTE);

const noteOf = (n: ThreadNote) => ({ follows: n.follows, threadRead: n.read });

/** Under a secure channel's line with a thread: who replied, how many, when last, and new ones in a thread you follow. */
export const SecureRepliesRow = memo(function SecureRepliesRow({
  instanceKey,
  thread,
  unread,
  hours,
  onOpen,
}: {
  instanceKey: string;
  thread: SecureThread;
  unread: number;
  hours: number;
  onOpen: (parent: number) => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  return (
    <motion.button
      type="button"
      onClick={() => onOpen(thread.parent)}
      initial={{ opacity: 0, y: -4 }}
      animate={{ opacity: 1, y: 0 }}
      transition={SPRING}
      whileTap={{ scale: 0.98 }}
      className="group/replies mt-1 -ml-1.5 flex max-w-full items-center gap-2 rounded-xl border border-transparent px-1.5 py-1 text-left text-xs transition-colors hover:border-border hover:bg-card/70"
    >
      <Faces instanceKey={instanceKey} ids={thread.participants} />
      <span className="flex shrink-0 items-center gap-1 font-bold text-primary">
        <T k="chat.threads.replies" values={{ count: <Count value={thread.replies} /> }} count={thread.replies} />
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
      <span className="grid min-w-0 text-muted-foreground">
        <span className={cn(SWAP, "group-hover/replies:-translate-y-1 group-hover/replies:opacity-0 group-focus-visible/replies:-translate-y-1 group-focus-visible/replies:opacity-0")}>
          {archived(thread, hours)
            ? t("chat.threads.archived")
            : thread.lastAt
              ? t("chat.threads.lastReply", { time: ago(lang, new Date(thread.lastAt)) })
              : t("chat.secureThreads.noReplies")}
        </span>
        <span aria-hidden className={cn(SWAP, "translate-y-1 opacity-0 group-hover/replies:translate-y-0 group-hover/replies:opacity-100 group-focus-visible/replies:translate-y-0 group-focus-visible/replies:opacity-100")}>
          {t("chat.threads.view")}
        </span>
      </span>
    </motion.button>
  );
});

/** On a reply also sent to the channel: in the channel, a way into its thread; in the thread, that it went to the channel too. */
export function SecureAlsoSent({ item, inThread, onOpen }: { item: Item; inThread: boolean; onOpen: (parent: number) => void }) {
  const { t } = useI18n();
  if (!item.inChannel || !item.thread) return null;
  if (inThread) return <p className="text-[0.7rem] font-bold text-muted-foreground">{t("chat.threads.alsoSent")}</p>;
  return (
    <button
      type="button"
      onClick={() => onOpen(item.thread!)}
      className="flex items-center gap-1 text-[0.7rem] font-bold text-muted-foreground transition-colors hover:text-primary"
    >
      <CornerDownRightIcon className="size-3" /> {t("chat.threads.repliedInThread")}
    </button>
  );
}

/** The top of a secure thread: its message, if this device has it, then how many replies follow. */
function SecureThreadStart({
  instanceKey,
  parent,
  count,
  author,
  member,
}: {
  instanceKey: string;
  parent: Item | undefined;
  count: number;
  author: User | undefined;
  member: Member | undefined;
}) {
  const { t } = useI18n();
  const display = usePrefs((p) => p.messageDisplay);
  return (
    <div className="pt-3">
      {parent ? (
        <div className="message-row first flex gap-3 px-4">
          <MessageLine display={display} first author={author} member={member} date={new Date(parent.at)} instanceKey={instanceKey}>
            <MessageBody content={lineText(parent)} display={display} />
          </MessageLine>
        </div>
      ) : (
        <p className="mx-4 rounded-2xl border border-dashed px-3 py-2.5 text-sm text-muted-foreground">
          {t("chat.secureThreads.parentMissing")}
        </p>
      )}
      <div role="separator" className="my-3 flex items-center gap-3 px-4 text-xs font-bold text-muted-foreground">
        <span>
          {count === 0 ? t("chat.threads.noReplies") : <T k="chat.threads.replies" values={{ count: <Count value={count} /> }} count={count} />}
        </span>
        <span className="h-px flex-1 bg-border" />
      </div>
    </div>
  );
}

/**
 * A secure channel's thread beside it: its message, its replies and lock
 * changes as this device opened them, and a composer of its own with "Also
 * send to #channel". Follow and lock (for moderators) sit in its header.
 */
export function SecureThreadPanel({
  instanceKey,
  serverId,
  channel,
  parent,
  org,
  me,
  userOf,
  memberOf,
  describe,
  canSend,
  canAttach,
  canModerate,
  onClose,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  parent: number;
  org: Organized;
  me: User;
  userOf: (id: string) => User | undefined;
  memberOf: (id: string) => Member | undefined;
  describe: (item: Item) => string;
  canSend: boolean;
  canAttach: boolean;
  canModerate: boolean;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const id = channel.id;
  const items = useFuwa((s) => s.instances[instanceKey]?.dms.items[id] ?? NO_ITEMS);
  const note = useThreadNote(instanceKey, id);
  const hours = useArchiveHours(instanceKey, serverId);
  const message = useMemo(() => items.find((i) => i.seq === parent && i.kind === "text" && !i.deleted), [items, parent]);
  const lines = org.inThread.get(parent) ?? NO_ITEMS;
  const thread = org.threads.get(parent);
  const followed = following(noteOf(note), parent, items, me.id);
  const locked = !!thread?.locked;
  const last = lines.at(-1)?.seq ?? 0;
  const pendingIn = useCallback((p: PendingMessage) => p.thread === parent, [parent]);
  const hooks = useMemo<ThreadHooks>(
    () => ({
      under: (i) => <SecureAlsoSent item={i} inThread onOpen={() => {}} />,
      canStart: () => false,
      start: () => {},
      has: () => true,
      kept: () => false,
    }),
    [],
  );

  // What's open is read.
  useEffect(() => {
    if (last && document.visibilityState === "visible") void markSecureThreadRead(instanceKey, id, parent, last);
  }, [instanceKey, id, parent, last]);

  const fail = (err: unknown) => toast(dmProblem(err));
  const follow = () =>
    followSecureThread(instanceKey, id, parent, !followed)
      .then(() => toast(followed ? t("chat.threads.unfollowed") : t("chat.threads.followed")))
      .catch(fail);
  const lock = () => lockSecureThread(instanceKey, id, parent, !locked).catch(fail);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <SecureThreadHeader
        channelName={channel.name}
        archived={archived(thread, hours)}
        locked={locked}
        followed={followed}
        canLock={canModerate && canSend}
        onFollow={follow}
        onLock={lock}
        onClose={onClose}
      />
      <EncryptedMessages
        key={parent}
        instanceKey={instanceKey}
        id={id}
        me={me}
        userOf={userOf}
        memberOf={memberOf}
        describe={describe}
        beginning={
          <SecureThreadStart
            instanceKey={instanceKey}
            parent={message}
            count={thread?.replies ?? 0}
            author={message && userOf(message.senderId)}
            member={message && memberOf(message.senderId)}
          />
        }
        canModerate={canModerate}
        deleteQuestion={t("chat.secure.deleteQuestion")}
        joiningText={t("chat.secure.joining")}
        lines={lines}
        pendingIn={pendingIn}
        threads={hooks}
      />
      <EncryptedComposer
        instanceKey={instanceKey}
        id={id}
        placeholder={t("chat.threads.placeholder")}
        promise={t("chat.secure.promise")}
        files={canAttach}
        locked={!canSend ? t("chat.secure.noPermission") : locked && !canModerate ? t("chat.secureThreads.locked") : ""}
        action={null}
        thread={{ parent, channelName: channel.name }}
      />
    </div>
  );
}

/** The thread's header: where it is and whether it's archived or locked, with follow, lock and close. */
function SecureThreadHeader({
  channelName,
  archived,
  locked,
  followed,
  canLock,
  onFollow,
  onLock,
  onClose,
}: {
  channelName: string;
  archived: boolean;
  locked: boolean;
  followed: boolean;
  canLock: boolean;
  onFollow: () => void;
  onLock: () => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  return (
    <header className="flex h-14 shrink-0 items-center gap-2 border-b px-3">
      <MessagesSquareIcon className="size-5 shrink-0 text-primary" />
      <div className="min-w-0 flex-1">
        <h2 className="truncate leading-tight font-extrabold">{t("chat.threads.thread")}</h2>
        <p className="truncate text-xs text-muted-foreground">
          {t(
            archived && locked
              ? "chat.threads.whereArchivedLocked"
              : archived
                ? "chat.threads.whereArchived"
                : locked
                  ? "chat.threads.whereLocked"
                  : "chat.threads.where",
            { channel: channelName },
          )}
        </p>
      </div>
      <PanelButton label={followed ? t("chat.threads.unfollow") : t("chat.threads.follow")} active={followed} onClick={onFollow}>
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
      {canLock && (
        <PanelButton label={locked ? t("chat.threads.unlock") : t("chat.threads.lock")} active={locked} onClick={onLock}>
          {locked ? <LockIcon /> : <LockOpenIcon />}
        </PanelButton>
      )}
      <PanelButton label={t("chat.threads.close")} onClick={onClose}>
        <XIcon />
      </PanelButton>
    </header>
  );
}

// Previews show text, not Markdown marks.
const plain = (content: string) =>
  content
    .replace(/[*_~`>#]+/g, "")
    .replace(/\s+/g, " ")
    .trim();

/** A secure channel's threads, the latest reply first, open or archived, searched on this device. */
export function SecureThreadList({
  instanceKey,
  serverId,
  channel,
  org,
  me,
  userOf,
  onOpen,
  onClose,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  org: Organized;
  me: User;
  userOf: (id: string) => User | undefined;
  onOpen: (parent: number) => void;
  onClose: () => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const [query, setQuery] = useState("");
  const [showArchived, setShowArchived] = useState(false);
  const hours = useArchiveHours(instanceKey, serverId);
  const items = useFuwa((s) => s.instances[instanceKey]?.dms.items[channel.id] ?? NO_ITEMS);
  const note = useThreadNote(instanceKey, channel.id);
  const bySeq = useMemo(() => new Map(items.map((i) => [i.seq, i])), [items]);
  const threads = useMemo(
    () => search(org, bySeq, query).filter((th) => archived(th, hours) === showArchived),
    [org, bySeq, query, hours, showArchived],
  );
  const tabs = hours > 0 ? [false, true] : [false];

  return (
    <div className="flex h-full min-h-0 flex-col">
      <ThreadListHeader where={t("chat.secureThreads.where", { channel: channel.name })} onClose={onClose} />
      <div className="flex flex-col gap-2 border-b p-3">
        <ThreadFilters query={query} onQuery={setQuery} tabs={tabs} archived={showArchived} onArchived={setShowArchived} glide={`secure-thread-tab-${channel.id}`} />
      </div>
      <div className="scroll-thin min-h-0 flex-1 overflow-y-auto p-2">
        {threads.length === 0 && (
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={SPRING}
            className="grid place-items-center gap-2 px-6 py-12 text-center"
          >
            <span className="float grid size-12 place-items-center rounded-full bg-primary/10 text-primary">
              <MessagesSquareIcon className="size-6" />
            </span>
            <p className="text-sm font-bold">{query ? t("chat.threads.noMatches") : showArchived ? t("chat.threads.noArchived") : t("chat.threads.none")}</p>
            <p className="text-xs text-muted-foreground">
              {query
                ? t("chat.secureThreads.searchNote")
                : showArchived
                  ? hours >= 48
                    ? t("chat.threads.archivedHintDays", { count: Math.round(hours / 24) })
                    : t("chat.threads.archivedHintHours", { count: hours })
                  : t("chat.threads.noneHint")}
            </p>
          </motion.div>
        )}
        <AnimatePresence initial={false}>
          {threads.map((th, n) => {
            const m = bySeq.get(th.parent);
            const author = m ? userOf(m.senderId) : undefined;
            const unread = following(noteOf(note), th.parent, items, me.id)
              ? unreadIn(noteOf(note), th.parent, org.inThread.get(th.parent) ?? NO_ITEMS, me.id)
              : 0;
            return (
              <motion.button
                key={th.parent}
                type="button"
                layout="position"
                initial={{ opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0 }}
                transition={{ ...SPRING, delay: Math.min(n, 8) * 0.025 }}
                onClick={() => onOpen(th.parent)}
                className="flex w-full gap-2.5 rounded-xl p-2.5 text-left transition-colors hover:bg-muted/70"
              >
                <UserAvatar user={author} className="size-8 text-xs" />
                <span className="min-w-0 flex-1">
                  <span className="flex items-baseline gap-2">
                    <b className="truncate text-sm">{m ? displayName(author) : t("chat.secureThreads.earlier")}</b>
                    {m && <span className="shrink-0 text-[0.7rem] text-muted-foreground">{ago(lang, new Date(m.at))}</span>}
                  </span>
                  <span className="line-clamp-2 text-sm break-words text-muted-foreground">{m ? plain(lineText(m)) || "…" : t("chat.secureThreads.notHere")}</span>
                  <span className="mt-1 flex items-center gap-2 text-xs">
                    <Faces instanceKey={instanceKey} ids={th.participants} size="size-4" />
                    <b className="text-primary">
                      {t("chat.threads.replies", { count: th.replies })}
                    </b>
                    {unread > 0 && (
                      <span className="rounded-full bg-primary px-1.5 text-[0.65rem] font-extrabold text-primary-foreground tabular-nums">
                        {unread > 99 ? t("chat.threads.newMany") : t("chat.threads.newCount", { count: unread })}
                      </span>
                    )}
                    {th.locked && <LockIcon className="size-3 text-muted-foreground" />}
                    {th.lastAt > 0 && <span className="truncate text-muted-foreground">{t("chat.threads.lastAgo", { time: ago(lang, new Date(th.lastAt)) })}</span>}
                  </span>
                </span>
              </motion.button>
            );
          })}
        </AnimatePresence>
      </div>
    </div>
  );
}
