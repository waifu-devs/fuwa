import {
  BadgeCheckIcon,
  BotOffIcon,
  ChevronLeftIcon,
  HistoryIcon,
  ImageOffIcon,
  LoaderIcon,
  LockKeyholeIcon,
  RotateCcwKeyIcon,
  SearchXIcon,
  ShieldCheckIcon,
  ShieldOffIcon,
  UserPlusIcon,
} from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { Permission, type Channel, type Member, type User } from "@/gen/fuwa/v1/types_pb";
import { SECURE_BROKEN, secureModerates } from "@/e2ee/engine";
import { canHaveThread, following, organize, unreadIn } from "@/e2ee/threads";
import type { Item } from "@/e2ee/vault";
import { focusChannel } from "@/fuwa/actions";
import { dmProblem, markDmRead, prepareSecureChannel, resetSecureChannel, setSecureHistory } from "@/fuwa/dms";
import { useAccess } from "@/fuwa/hooks";
import { useFuwa, type DmMember, type DmState, type PendingMessage } from "@/fuwa/store";
import { NotificationBell } from "@/components/chat/NotificationBell";
import { EncryptedComposer, EncryptedMessages, Starting, Unavailable, type ThreadHooks } from "@/components/dm/DmView";
import { earlierFrom, type Earlier } from "@/components/dm/earlier";
import { SecureAlsoSent, SecureRepliesRow, SecureThreadList, SecureThreadPanel, useArchiveHours, useThreadNote } from "@/components/chat/SecureThreads";
import { ThreadsButton } from "@/components/chat/Threads";
import { Padlock } from "@/components/dm/Padlock";
import { ConnDot, UserAvatar } from "@/components/Icons";
import { connectionLabel } from "@/components/icons-utils";
import { InlineMarkdown } from "@/components/Markdown";
import { SwapText } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { useLayout } from "@/components/Shell";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { type Key, T, useI18n } from "@/i18n/react";
import { displayName, memberName, type Lang } from "@/lib/format";
import { hasIn } from "@/lib/permissions";
import { listNames } from "@/lib/shared";
import { setTitle } from "@/lib/notify";
import type { ThreadPanelState } from "@/lib/threads";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

const NO_MEMBERS: DmMember[] = [];
const NO_SERVER_MEMBERS: Member[] = [];
const NO_ITEMS: Item[] = [];
/** The channel's own list shows messages being sent there, not those going only to a thread. */
const inChannel = (p: PendingMessage) => !p.thread || !!p.inChannel;

/** What a secure channel can't do, said once: the server can't read it, so nothing that needs to can work. */
const CANT: { icon: typeof ShieldOffIcon; text: Key }[] = [
  { icon: ShieldOffIcon, text: "chat.secure.cant.automod" },
  { icon: SearchXIcon, text: "chat.secure.cant.search" },
  { icon: BotOffIcon, text: "chat.secure.cant.bots" },
  { icon: ImageOffIcon, text: "chat.secure.cant.links" },
];

const LATER: Record<"on" | "off", Key> = {
  off: "chat.secure.later.off",
  on: "chat.secure.later.on",
};

/**
 * A secure channel: a server's channel whose messages are end-to-end
 * encrypted between the devices of the people who can see it. It reads
 * and writes through this browser's encryption (`@/e2ee/engine`), like a
 * direct message, and never through the server's messages.
 */
export function SecureChannelView({ instanceKey, serverId, channel }: { instanceKey: string; serverId: string; channel: Channel }) {
  const lang = useI18n();
  const { t } = lang;
  const status = useFuwa((s) => s.instances[instanceKey]?.dms.status ?? "off");
  const me = useFuwa((s) => s.instances[instanceKey]?.me ?? undefined);
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? NO_SERVER_MEMBERS);
  const access = useAccess(instanceKey, serverId);
  const [info, setInfo] = useState(false);
  const id = channel.id;

  useFocused(instanceKey, serverId, channel);

  const canSend = hasIn(access, id, Permission.SEND_MESSAGES);
  const canAttach = canSend && hasIn(access, id, Permission.ATTACH_FILES);
  const canReset = hasIn(access, id, Permission.MANAGE_CHANNELS);
  const sharesHistory = useFuwa((s) => !!s.instances[instanceKey]?.dms.secureHistory[id]);

  // Once encryption is running here: catch up, and if you may write, bring in everyone who can see the channel.
  useEffect(() => {
    if (status === "ready") void prepareSecureChannel(instanceKey, serverId, id, canSend).catch(() => {});
  }, [status, instanceKey, serverId, id, canSend]);

  const byId = useMemo(() => new Map(members.map((m) => [m.user?.id ?? "", m])), [members]);
  const userOf = useCallback((userId: string) => byId.get(userId)?.user ?? users?.[userId], [byId, users]);
  const memberOf = useCallback((userId: string) => byId.get(userId), [byId]);
  const earlier = useFuwa((s) => {
    const dms = s.instances[instanceKey]?.dms;
    return earlierFrom(dms?.items[id], dms?.backup.status === "locked");
  });
  const describe = useCallback(
    (item: Item) => (me ? channelLine(lang, item, (u) => nameIn(byId, users, u), me, item.kind === "joined" ? earlier : null) : ""),
    [lang, byId, users, me, earlier],
  );

  // ── Threads, worked out from what this device opened (the server can't tell a reply from any other line).
  const items = useFuwa((s) => s.instances[instanceKey]?.dms.items[id] ?? NO_ITEMS);
  const roles = useFuwa((s) => s.instances[instanceKey]?.roles[serverId]);
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[serverId]);
  // Read again whenever who has which roles, or the channel's overwrites, change.
  const moderates = useMemo(
    () => (members && roles && channels ? secureModerates(instanceKey, serverId, id) : () => false),
    [instanceKey, serverId, id, members, roles, channels],
  );
  const org = useMemo(() => organize(items, moderates), [items, moderates]);
  const [panel, setPanel] = useState<ThreadPanelState>(null);
  const [panelFor, setPanelFor] = useState(id);
  if (panelFor !== id) {
    setPanelFor(id);
    setPanel(null);
  }
  const openThread = useCallback((parent: number) => setPanel({ kind: "thread", id: String(parent) }), []);
  const closePanel = useCallback(() => setPanel(null), []);
  const canStart = hasIn(access, id, Permission.CREATE_THREADS);
  const canModerate = hasIn(access, id, Permission.MANAGE_MESSAGES);
  const hooks = useThreadHooks({ instanceKey, serverId, id, org, items, me, openThread, canSend, canStart });

  const ready = !!me && status === "ready";
  const side =
    panel?.kind === "thread" && me ? (
      <SecureThreadPanel
        key={panel.id}
        instanceKey={instanceKey}
        serverId={serverId}
        channel={channel}
        parent={Number(panel.id)}
        org={org}
        me={me}
        userOf={userOf}
        memberOf={memberOf}
        describe={describe}
        canSend={canSend}
        canAttach={canAttach}
        canModerate={canModerate}
        onClose={closePanel}
      />
    ) : panel && me ? (
      <SecureThreadList
        instanceKey={instanceKey}
        serverId={serverId}
        channel={channel}
        org={org}
        me={me}
        userOf={userOf}
        onOpen={openThread}
        onClose={closePanel}
      />
    ) : null;

  return (
    <div className="relative flex h-full min-h-0">
      <div className="flex min-w-0 flex-1 flex-col">
        <SecureHeader
          instanceKey={instanceKey}
          serverId={serverId}
          channel={channel}
          threads={status === "ready"}
          panel={panel}
          onPanel={setPanel}
          onInfo={() => setInfo(true)}
        />
        {ready && me ? (
          <>
            <EncryptedMessages
              instanceKey={instanceKey}
              id={id}
              me={me}
              userOf={userOf}
              memberOf={memberOf}
              describe={describe}
              beginning={<SecureBeginning channel={channel} sharesHistory={sharesHistory} />}
              canModerate={canModerate}
              deleteQuestion={t("chat.secure.deleteQuestion")}
              joiningText={t("chat.secure.joining")}
              lines={org.channel}
              pendingIn={inChannel}
              threads={hooks}
            />
            <SecureComposer instanceKey={instanceKey} serverId={serverId} channel={channel} canSend={canSend} canAttach={canAttach} canReset={canReset} />
            <SecureChannelDialog
              open={info}
              onOpenChange={setInfo}
              instanceKey={instanceKey}
              serverId={serverId}
              channel={channel}
              byId={byId}
              canReset={canReset}
              canSend={canSend}
            />
          </>
        ) : (
          <NotReady instanceKey={instanceKey} />
        )}
      </div>
      <Side>{ready && side}</Side>
    </div>
  );
}

/** While it's open: the channel is the one in focus, read, and named in the page's title. */
function useFocused(instanceKey: string, serverId: string, channel: Channel) {
  const { t } = useI18n();
  const serverName = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.name);
  const id = channel.id;
  useEffect(() => {
    focusChannel(instanceKey, id);
    void markDmRead(instanceKey, id);
    return () => focusChannel(null, null);
  }, [instanceKey, id]);
  useEffect(() => {
    setTitle(t("chat.channel.pageTitle", { channel: channel.name, server: serverName ?? "fuwa" }));
  }, [channel.name, serverName, t]);
  useEffect(() => () => setTitle("fuwa"), []);
}

/** How the channel's lines show their threads: the replies row under a parent, and who may start one. */
function useThreadHooks({
  instanceKey,
  serverId,
  id,
  org,
  items,
  me,
  openThread,
  canSend,
  canStart,
}: {
  instanceKey: string;
  serverId: string;
  id: string;
  org: ReturnType<typeof organize>;
  items: Item[];
  me: User | undefined;
  openThread: (parent: number) => void;
  canSend: boolean;
  canStart: boolean;
}) {
  const note = useThreadNote(instanceKey, id);
  const hours = useArchiveHours(instanceKey, serverId);
  return useMemo<ThreadHooks>(() => {
    const threadNote = { follows: note.follows, threadRead: note.read };
    return {
      under: (item): ReactNode => {
        if (item.thread) return <SecureAlsoSent item={item} inThread={false} onOpen={openThread} />;
        const thread = org.threads.get(item.seq);
        if (!thread || !me) return null;
        const unread = following(threadNote, thread.parent, items, me.id)
          ? unreadIn(threadNote, thread.parent, org.inThread.get(thread.parent) ?? NO_ITEMS, me.id)
          : 0;
        return <SecureRepliesRow instanceKey={instanceKey} thread={thread} unread={unread} hours={hours} onOpen={openThread} />;
      },
      // A thread starts with Start threads; replying in one that's there takes only Send messages.
      canStart: (item) => canHaveThread(item) && canSend && (canStart || org.threads.has(item.seq)),
      start: (item) => openThread(item.seq),
      has: (item) => org.threads.has(item.seq),
      kept: (item) => (org.inThread.get(item.seq) ?? NO_ITEMS).some((r) => r.kind === "text" && !r.deleted && r.senderId !== item.senderId),
    };
  }, [org, items, note, me, instanceKey, hours, openThread, canSend, canStart]);
}

/** The channel's name and topic, the connection when it isn't live, and its buttons. */
function SecureHeader({
  instanceKey,
  serverId,
  channel,
  threads,
  panel,
  onPanel,
  onInfo,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  /** Whether threads can be shown yet (encryption is running). */
  threads: boolean;
  panel: ThreadPanelState;
  onPanel: (panel: ThreadPanelState) => void;
  onInfo: () => void;
}) {
  const { t } = useI18n();
  const { compact, setNavOpen } = useLayout();
  const connection = useFuwa((s) => s.instances[instanceKey]?.connection ?? "connecting");
  const id = channel.id;
  return (
  <header className="flex h-14 shrink-0 items-center gap-2 border-b px-2 sm:px-4">
    {compact && (
      <button
        type="button"
        aria-label={t("chat.channel.channels")}
        onClick={() => setNavOpen(true)}
        className="grid size-9 place-items-center rounded-full text-muted-foreground transition hover:-translate-x-0.5 hover:bg-muted"
      >
        <ChevronLeftIcon className="size-5" />
      </button>
    )}
    <AnimatePresence mode="popLayout" initial={false}>
      <motion.span
        key={id}
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: -10 }}
        transition={SPRING}
        className="flex min-w-0 shrink items-center gap-2"
      >
        <ShieldCheckIcon className="size-5 shrink-0 text-emerald-600 dark:text-emerald-400" />
        <h1 className="truncate font-extrabold">
          <SwapText className="truncate align-bottom">{channel.name}</SwapText>
        </h1>
      </motion.span>
    </AnimatePresence>
    {channel.topic && (
      <>
        <span className="hidden h-5 w-px bg-border sm:block" />
        <InlineMarkdown className="hidden min-w-0 truncate text-sm text-muted-foreground sm:block">{channel.topic}</InlineMarkdown>
      </>
    )}
    <span className="flex-1" />
    <AnimatePresence>
      {connection !== "live" && (
        <motion.span
          initial={{ opacity: 0, scale: 0.9 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.9 }}
          className="flex items-center gap-1.5 rounded-full bg-muted px-2.5 py-1 text-xs font-bold text-muted-foreground"
        >
          <ConnDot state={connection} /> {connectionLabel(connection)}
        </motion.span>
      )}
    </AnimatePresence>
    <NotificationBell instanceKey={instanceKey} serverId={serverId} channel={channel} />
    {threads && (
      <ThreadsButton
        open={panel?.kind === "threads"}
        active={!!panel}
        onClick={() => onPanel(panel?.kind === "threads" ? null : { kind: "threads" })}
      />
    )}
    <motion.button
      type="button"
      onClick={onInfo}
      whileTap={{ scale: 0.92 }}
      initial={{ opacity: 0, scale: 0.8 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={SPRING}
      title={t("chat.secure.seeWho")}
      className="group relative flex shrink-0 items-center gap-1.5 overflow-hidden rounded-full bg-emerald-500/12 px-2.5 py-1 text-xs font-bold text-emerald-700 transition-colors hover:bg-emerald-500/20 dark:text-emerald-300"
    >
      <span aria-hidden className="shine pointer-events-none absolute inset-0" />
      <LockKeyholeIcon className="size-3.5 transition-transform duration-300 group-hover:-rotate-12 group-hover:scale-110" />
      <span className="hidden sm:inline">{t("chat.secure.encrypted")}</span>
    </motion.button>
  </header>
  );
}

/** The composer, or why you can't write; a broken channel offers starting over to those who may. */
function SecureComposer({
  instanceKey,
  serverId,
  channel,
  canSend,
  canAttach,
  canReset,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  canSend: boolean;
  canAttach: boolean;
  canReset: boolean;
}) {
  const { t } = useI18n();
  const id = channel.id;
  const broken = useFuwa((s) => s.instances[instanceKey]?.dms.blocked[id] === SECURE_BROKEN);
  const reset = canReset ? <ResetButton instanceKey={instanceKey} serverId={serverId} channelId={id} write={canSend} /> : null;
  return (
    <EncryptedComposer
      instanceKey={instanceKey}
      id={id}
      placeholder={t("chat.channel.placeholder", { channel: channel.name })}
      promise={t("chat.secure.promise")}
      files={canAttach}
      dropTo={channel.name}
      locked={canSend || broken ? "" : t("chat.secure.noPermission")}
      action={broken ? reset : undefined}
    />
  );
}

/** Before encryption runs here: starting, or why it can't. */
function NotReady({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const status = useFuwa((s) => s.instances[instanceKey]?.dms.status ?? "off");
  const problem = useFuwa((s) => s.instances[instanceKey]?.dms.problem ?? null);
  if (status === "unsupported" || status === "failed") return <Unavailable text={problem ?? t("chat.secure.unavailable")} />;
  return <Starting />;
}

/** Threads beside the channel on a wide screen, or over it on a narrow one. */
function Side({ children }: { children: ReactNode }) {
  const docked = useMediaQuery("(min-width: 1024px)");
  return (
    <AnimatePresence initial={false} mode="popLayout">
      {children &&
        (docked ? (
          <motion.aside
            key="threads"
            initial={{ x: 32, opacity: 0 }}
            animate={{ x: 0, opacity: 1 }}
            exit={{ x: 32, opacity: 0 }}
            transition={{ type: "spring", stiffness: 400, damping: 40 }}
            className="surface-side h-full w-[400px] shrink-0 overflow-hidden border-l xl:w-[440px]"
          >
            {children}
          </motion.aside>
        ) : (
          // On a narrow screen a thread takes the whole width, sliding over the channel.
          <motion.aside
            key="threads-sheet"
            initial={{ x: "100%" }}
            animate={{ x: 0 }}
            exit={{ x: "100%" }}
            transition={{ type: "spring", stiffness: 420, damping: 40 }}
            className="surface-side absolute inset-0 z-30 flex flex-col shadow-2xl"
          >
            {children}
          </motion.aside>
        ))}
    </AnimatePresence>
  );
}

function nameIn(byId: Map<string, Member>, users: Record<string, User> | undefined, userId: string) {
  const member = byId.get(userId);
  return member ? memberName(member) : displayName(users?.[userId]);
}

/** What changed about the channel's devices, in words, from the commit itself (not from the server). */
function channelLine(lang: Lang, item: Item, nameOf: (userId: string) => string, me: User, earlier: Earlier = null): string {
  const { t } = lang;
  const capital = (text: string) => `${text[0]?.toUpperCase() ?? ""}${text.slice(1)}`;
  const mine = item.senderId === me.id;
  /** A line about what the sender did: theirs by name, or yours. */
  const said = (theirs: Key, yours: Key, values: Record<string, string> = {}) =>
    capital(mine ? t(yours, values) : t(theirs, { ...values, name: nameOf(item.senderId) }));
  if (item.kind === "joined") {
    if (earlier === "shared") return t("chat.secure.line.joinedShared");
    if (earlier === "backup") return t("chat.secure.line.joinedBackup");
    if (earlier === "restorable") return t("chat.secure.line.joinedRestorable");
    return t("chat.secure.line.joined");
  }
  if (item.kind === "unreadable") return mine ? t("chat.secure.line.unreadableMine") : t("chat.secure.line.unreadable", { name: nameOf(item.senderId) });
  if (item.kind === "setting") {
    return item.content === "on"
      ? said("chat.secure.line.historyOn", "chat.secure.line.historyOnMine")
      : said("chat.secure.line.historyOff", "chat.secure.line.historyOffMine");
  }
  if (item.kind === "thread") {
    return item.content === "locked"
      ? said("chat.secure.line.locked", "chat.secure.line.lockedMine")
      : said("chat.secure.line.unlocked", "chat.secure.line.unlockedMine");
  }
  if (item.kind === "reset") return said("chat.secure.line.reset", "chat.secure.line.resetMine");
  const devices = (list: Item["added"]) =>
    listNames(
      lang,
      [...new Set(list.map((d) => d.userId))].map((userId) => {
        const count = list.filter((d) => d.userId === userId).length;
        return userId === me.id ? t("chat.secure.line.yourDevices", { count }) : t("chat.secure.line.theirDevices", { count, name: nameOf(userId) });
      }),
    );
  const added = devices(item.added);
  const removed = devices(item.removed);
  const alone = item.added.length === 1 && item.added[0].userId === item.senderId && !item.removed.length;
  if (item.seq === 1) {
    return added
      ? said("chat.secure.line.started", "chat.secure.line.startedMine", { added })
      : said("chat.secure.line.startedAlone", "chat.secure.line.startedAloneMine");
  }
  if (alone) return said("chat.secure.line.newDevice", "chat.secure.line.newDeviceMine");
  if (added && removed) return said("chat.secure.line.addedRemoved", "chat.secure.line.addedRemovedMine", { added, removed });
  if (added) return said("chat.secure.line.added", "chat.secure.line.addedMine", { added });
  if (removed) return said("chat.secure.line.removed", "chat.secure.line.removedMine", { removed });
  return said("chat.secure.line.refreshed", "chat.secure.line.refreshedMine");
}

function SecureBeginning({ channel, sharesHistory }: { channel: Channel; sharesHistory: boolean }) {
  const { t } = useI18n();
  return (
    <motion.div initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} transition={{ ...SPRING, delay: 0.05 }} className="px-4 pt-10 pb-4">
      <span className="relative inline-grid size-16 place-items-center rounded-2xl bg-emerald-500/15 text-emerald-600 dark:text-emerald-400">
        <ShieldCheckIcon className="size-8" />
        <motion.span
          initial={{ scale: 0, rotate: -30 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.45 }}
          className="absolute -right-1.5 -bottom-1.5 grid size-7 place-items-center rounded-full bg-emerald-500 text-white ring-4 ring-card"
        >
          <LockKeyholeIcon className="size-3.5" />
        </motion.span>
      </span>
      <h2 className="mt-3 text-2xl font-extrabold sm:text-3xl">{t("chat.beginning.title", { channel: channel.name })}</h2>
      <p className="mt-3 flex max-w-xl items-start gap-2 rounded-2xl bg-emerald-500/10 px-3 py-2.5 text-sm text-emerald-900 dark:text-emerald-100">
        <LockKeyholeIcon className="mt-0.5 size-4 shrink-0 text-emerald-600 dark:text-emerald-400" />
        <span>
          <T k="chat.secure.beginning.about" values={{ secure: <b>{t("chat.secure.beginning.secureChannel")}</b> }} />{" "}
          {sharesHistory ? t("chat.secure.beginning.history") : t("chat.secure.beginning.noHistory")}
        </span>
      </p>
    </motion.div>
  );
}

/** Whether you checked someone's safety number in a direct message, and every device of theirs in this channel was in it. */
function verifiedPeople(dms: DmState, meId: string, channelMembers: DmMember[]): Set<string> {
  const hex = (key: Uint8Array) => Array.from(key, (b) => b.toString(16).padStart(2, "0")).join("");
  const out = new Set<string>();
  for (const userId of new Set(channelMembers.map((m) => m.userId))) {
    if (userId === meId) continue;
    const c = dms.conversations.find((x) => x.users.some((u) => u.id === userId) && x.users.some((u) => u.id === meId));
    if (!c || !dms.safety[c.id] || dms.verified[c.id] !== dms.safety[c.id]) continue;
    const checked = new Set((dms.members[c.id] ?? []).filter((m) => m.userId === userId).map((m) => hex(m.signatureKey)));
    if (channelMembers.filter((m) => m.userId === userId).every((m) => checked.has(hex(m.signatureKey)))) out.add(userId);
  }
  return out;
}

/** How a secure channel is kept private: what the server can't do, and every person (and how many devices) that can read it. */
function SecureChannelDialog({
  open,
  onOpenChange,
  instanceKey,
  serverId,
  channel,
  byId,
  canReset,
  canSend,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  serverId: string;
  channel: Channel;
  byId: Map<string, Member>;
  canReset: boolean;
  canSend: boolean;
}) {
  const { t } = useI18n();
  const me = useFuwa((s) => s.instances[instanceKey]?.me);
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  const dms = useFuwa((s) => s.instances[instanceKey]?.dms);
  const members = dms?.members[channel.id] ?? NO_MEMBERS;
  const people = useMemo(() => {
    const counts = new Map<string, number>();
    for (const m of members) counts.set(m.userId, (counts.get(m.userId) ?? 0) + 1);
    return [...counts].sort(([a], [b]) => nameIn(byId, users, a).localeCompare(nameIn(byId, users, b)));
  }, [members, byId, users]);
  const verified = useMemo(() => (dms && me ? verifiedPeople(dms, me.id, members) : new Set<string>()), [dms, me, members]);
  const sharesHistory = !!dms?.secureHistory[channel.id];
  const [saving, setSaving] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const toggleHistory = (on: boolean) => {
    setSaving(true);
    setHistoryError(null);
    setSecureHistory(instanceKey, serverId, channel.id, on)
      .catch((err: unknown) => setHistoryError(dmProblem(err)))
      .finally(() => setSaving(false));
  };
  const lines = [...CANT, { icon: UserPlusIcon, text: sharesHistory ? LATER.on : LATER.off }];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <div className="mb-4 flex justify-center">
          <Padlock delay={0.1} />
        </div>
        <DialogHeader title={t("chat.secure.dialog.title", { channel: channel.name })} description={t("chat.secure.dialog.description")} />
        <ul className="grid gap-1.5 rounded-2xl border bg-muted/40 p-3 text-sm">
          {lines.map(({ icon: Icon, text }, n) => (
            <motion.li
              key={text}
              initial={{ opacity: 0, x: -8 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ ...SPRING, delay: 0.1 + n * 0.04 }}
              className="flex items-center gap-2.5 text-muted-foreground"
            >
              <Icon className="size-4 shrink-0" /> {t(text)}
            </motion.li>
          ))}
        </ul>
        <p className="mt-5 mb-2 text-sm font-extrabold">
          <T
            k="chat.secure.dialog.whoCanRead"
            values={{ count: <span className="font-bold text-muted-foreground">{t("chat.secure.dialog.peopleCount", { count: people.length })}</span> }}
          />
        </p>
        <ul className="scroll-thin -mx-1 max-h-64 overflow-y-auto px-1">
          {people.map(([userId, count], n) => {
            const user = byId.get(userId)?.user ?? users?.[userId];
            return (
              <motion.li
                key={userId}
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ ...SPRING, delay: Math.min(n, 10) * 0.03 }}
                className="flex items-center gap-3 rounded-xl px-2 py-1.5 hover:bg-muted/60"
              >
                <UserAvatar user={user} className="size-8 text-xs" />
                <span className="min-w-0 flex-1">
                  <span className="flex items-center gap-1.5 truncate font-bold">
                    {userId === me?.id ? t("chat.secure.dialog.you") : nameIn(byId, users, userId)}
                    {verified.has(userId) && (
                      <BadgeCheckIcon className="size-4 text-emerald-500" aria-label={t("chat.secure.dialog.verified")} />
                    )}
                  </span>
                  <span className="block text-xs text-muted-foreground">
                    {verified.has(userId) ? t("chat.secure.dialog.devicesVerified", { count }) : t("chat.secure.dialog.devices", { count })}
                  </span>
                </span>
              </motion.li>
            );
          })}
        </ul>
        <p className={cn("mt-4 text-xs text-muted-foreground")}>
          {t("chat.secure.dialog.whoNote")}
        </p>
        {canReset && (
          <label className="mt-4 flex cursor-pointer items-start gap-3 rounded-2xl border px-3 py-2.5 transition-colors hover:bg-muted/40">
            <HistoryIcon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1">
              <span className="block text-sm font-bold">{t("chat.secure.dialog.shareHistory")}</span>
              <span className="block text-xs text-muted-foreground">{t("chat.secure.dialog.shareHistoryAbout")}</span>
              {historyError && <span className="mt-1 block text-xs text-destructive">{historyError}</span>}
            </span>
            <Switch checked={sharesHistory} disabled={saving} onCheckedChange={toggleHistory} aria-label={t("chat.secure.dialog.shareHistory")} />
          </label>
        )}
        {canReset && (
          <div className="mt-3 flex items-center gap-3 rounded-2xl border border-dashed px-3 py-2.5">
            <p className="min-w-0 flex-1 text-xs text-muted-foreground">
              {t("chat.secure.dialog.resetAbout")}
            </p>
            <ResetButton instanceKey={instanceKey} serverId={serverId} channelId={channel.id} write={canSend} onDone={() => onOpenChange(false)} />
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

/** Starts the channel's encryption over (Manage Channels), after asking once. */
function ResetButton({
  instanceKey,
  serverId,
  channelId,
  write,
  onDone,
}: {
  instanceKey: string;
  serverId: string;
  channelId: string;
  write: boolean;
  onDone?: () => void;
}) {
  const { t } = useI18n();
  const [stage, setStage] = useState<"idle" | "ask" | "busy">("idle");
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (stage !== "ask") return;
    const timer = setTimeout(() => setStage("idle"), 4000);
    return () => clearTimeout(timer);
  }, [stage]);
  const go = () => {
    if (stage === "idle") return setStage("ask");
    if (stage !== "ask") return;
    setStage("busy");
    setError(null);
    resetSecureChannel(instanceKey, serverId, channelId, write)
      .then(() => onDone?.())
      .catch((err: unknown) => setError(dmProblem(err)))
      .finally(() => setStage("idle"));
  };
  return (
    <span className="flex shrink-0 flex-col items-end gap-1">
      <motion.button
        type="button"
        onClick={go}
        disabled={stage === "busy"}
        whileTap={{ scale: 0.94 }}
        layout
        transition={SPRING}
        className={cn(
          "group flex items-center gap-1.5 rounded-xl px-3 py-1.5 text-xs font-bold transition-colors",
          stage === "ask" ? "bg-destructive/12 text-destructive hover:bg-destructive/20" : "text-primary hover:bg-primary/10",
        )}
      >
        {stage === "busy" ? (
          <LoaderIcon className="size-3.5 animate-spin" />
        ) : (
          <RotateCcwKeyIcon className="size-3.5 transition-transform duration-500 group-hover:-rotate-45" />
        )}
        <SwapText>{stage === "ask" ? t("chat.secure.resetAsk") : t("chat.secure.reset")}</SwapText>
      </motion.button>
      {error && <span className="max-w-56 text-right text-[0.7rem] text-destructive">{error}</span>}
    </span>
  );
}
