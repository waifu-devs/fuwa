import { useNavigate } from "@tanstack/react-router";
import {
  BanIcon,
  CheckIcon,
  ChevronLeftIcon,
  EllipsisVerticalIcon,
  HeartHandshakeIcon,
  LoaderCircleIcon,
  MessageCircleIcon,
  SearchIcon,
  ShieldOffIcon,
  SparklesIcon,
  UserMinusIcon,
  UserPlusIcon,
  UsersIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type FormEvent } from "react";
import type { Friend } from "@/gen/fuwa/v1/friend_pb";
import { run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { acceptFriend, blockUser, removeFriend, sendFriendRequest, unblockUser } from "@/fuwa/friends";
import { openConversation } from "@/fuwa/dms";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { Count, SPRING } from "@/components/motion";
import { ProfilePopover } from "@/components/ProfilePopover";
import { useLayout } from "@/components/Shell";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { displayName, shownStatus } from "@/lib/format";
import { BLOCKED, cleanUsername, FRIEND, inTab, INCOMING, OUTGOING, pendingLine, waitingForYou, type FriendsTab } from "@/lib/friends";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { type Key, T, useI18n } from "@/i18n/react";

const NONE: Friend[] = [];

const TABS: { id: FriendsTab; label: Key }[] = [
  { id: "online", label: "dms-calls.friends.tab.online" },
  { id: "all", label: "dms-calls.friends.tab.all" },
  { id: "pending", label: "dms-calls.friends.tab.pending" },
  { id: "blocked", label: "dms-calls.friends.tab.blocked" },
];

/** Lines drawn past the edges, so a quick scroll doesn't show blank space. */
const OVERSCAN = 6;

/**
 * Your friends on an instance: who's online, everyone, requests waiting
 * either way and who you blocked, and a box to ask someone by username.
 * Only you ever see this list. Long lists draw only the lines in view.
 */
export function FriendsPage({ instanceKey }: { instanceKey: string }) {
  const list = useFuwa((s) => s.instances[instanceKey]?.friends.list ?? NONE);
  const status = useFuwa((s) => s.instances[instanceKey]?.friends.status ?? "off");
  const signedIn = useFuwa((s) => !!s.instances[instanceKey]?.me);
  const { compact, setNavOpen } = useLayout();
  const [tab, setTab] = useState<FriendsTab>(() => (waitingForYou(list) > 0 ? "pending" : "online"));
  const [adding, setAdding] = useState(false);
  const { t } = useI18n();
  const counts = useMemo(
    () => ({ online: inTab(list, "online").length, all: inTab(list, "all").length, pending: waitingForYou(list), blocked: inTab(list, "blocked").length }),
    [list],
  );

  return (
    <div className="flex h-full flex-col">
      <header className="flex h-14 shrink-0 items-center gap-2 border-b px-2 sm:px-4">
        {compact && (
          <button type="button" onClick={() => setNavOpen(true)} aria-label={t("common.back")} className="grid size-9 place-items-center rounded-full text-muted-foreground hover:bg-muted">
            <ChevronLeftIcon className="size-5" />
          </button>
        )}
        <h1 className="flex shrink-0 items-center gap-2 font-extrabold">
          <UsersIcon className="size-5 text-primary" />
          <span className={cn(compact && "sr-only")}>{t("dms-calls.friends.title")}</span>
        </h1>
        <span aria-hidden className={cn("mx-1 h-6 w-px bg-border", compact && "hidden")} />
        <Tabs instanceKey={instanceKey} tab={tab} onTab={setTab} counts={counts} />
        <motion.button
          type="button"
          whileTap={{ scale: 0.94 }}
          onClick={() => setAdding((a) => !a)}
          aria-expanded={adding}
          aria-label={adding ? t("common.close") : t("dms-calls.friends.add")}
          className={cn(
            "group flex shrink-0 items-center gap-1.5 rounded-full px-3 py-1.5 text-sm font-bold transition",
            adding ? "bg-muted text-foreground" : "bg-primary text-primary-foreground hover:brightness-110",
          )}
        >
          <motion.span animate={{ rotate: adding ? 45 : 0 }} transition={SPRING} className="grid">
            {adding ? <XIcon className="size-4" /> : <UserPlusIcon className="size-4 transition-transform duration-300 group-hover:scale-110" />}
          </motion.span>
          <span className="hidden sm:inline">{adding ? t("common.close") : t("dms-calls.friends.add")}</span>
        </motion.button>
      </header>

      <AnimatePresence initial={false}>
        {adding && (
          <motion.div
            key="add"
            initial={{ opacity: 0, y: -12 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -12, transition: { duration: 0.12 } }}
            transition={SPRING}
            className="shrink-0 border-b"
          >
            <AddFriend instanceKey={instanceKey} />
          </motion.div>
        )}
      </AnimatePresence>

      {signedIn && status === "unsupported" && (
        <Empty icon={<SparklesIcon className="size-7" />} title={t("dms-calls.friends.page.unsupportedTitle")} text={t("dms-calls.friends.page.unsupportedText")} />
      )}
      {signedIn && status !== "unsupported" && (
        <FriendsList instanceKey={instanceKey} tab={tab} list={list} loading={status === "loading"} onAdd={() => setAdding(true)} />
      )}
    </div>
  );
}

/** A tab's search, heading and lines. */
function FriendsList({
  instanceKey,
  tab,
  list,
  loading,
  onAdd,
}: {
  instanceKey: string;
  tab: FriendsTab;
  list: Friend[];
  loading: boolean;
  onAdd: () => void;
}) {
  const [query, setQuery] = useState("");
  const { t } = useI18n();
  const shown = useMemo(() => inTab(list, tab, query), [list, tab, query]);
  return (
    <>
      <div className="shrink-0 px-3 pt-3 sm:px-6">
        <label className="flex items-center gap-2 rounded-xl border bg-card px-3 py-2 text-sm transition focus-within:border-primary/60 focus-within:ring-2 focus-within:ring-primary/20">
          <SearchIcon className="size-4 text-muted-foreground" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("dms-calls.friends.page.search")}
            aria-label={t("dms-calls.friends.page.searchLabel")}
            className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground"
          />
          <AnimatePresence>
            {query && (
              <motion.button
                type="button"
                initial={{ scale: 0 }}
                animate={{ scale: 1 }}
                exit={{ scale: 0 }}
                onClick={() => setQuery("")}
                aria-label={t("dms-calls.friends.page.clearSearch")}
                className="grid size-5 place-items-center rounded-full text-muted-foreground hover:bg-muted"
              >
                <XIcon className="size-3.5" />
              </motion.button>
            )}
          </AnimatePresence>
        </label>
        <p className="mt-4 mb-1 px-1 text-xs font-bold tracking-wide text-muted-foreground uppercase">
          <T k="dms-calls.friends.page.heading" values={{ tab: t(TABS.find((x) => x.id === tab)!.label), count: <Count value={shown.length} /> }} count={shown.length} />
        </p>
      </div>
      <AnimatePresence mode="wait" initial={false}>
        <motion.div
          key={tab}
          initial={{ opacity: 0, x: 12 }}
          animate={{ opacity: 1, x: 0 }}
          exit={{ opacity: 0, x: -12 }}
          transition={{ duration: 0.16, ease: [0.22, 1, 0.36, 1] }}
          className="min-h-0 flex-1"
        >
          {loading && list.length === 0 ? (
            <Loading />
          ) : shown.length === 0 ? (
            <TabEmpty tab={tab} searching={!!query.trim()} onAdd={onAdd} />
          ) : (
            <FriendLines instanceKey={instanceKey} friends={shown} />
          )}
        </motion.div>
      </AnimatePresence>
    </>
  );
}

/** The four tabs, with the selection gliding between them and the requests waiting counted. */
function Tabs({
  instanceKey,
  tab,
  onTab,
  counts,
}: {
  instanceKey: string;
  tab: FriendsTab;
  onTab: (tab: FriendsTab) => void;
  counts: Record<FriendsTab, number>;
}) {
  const { t: tr } = useI18n();
  return (
    <nav aria-label={tr("dms-calls.friends.title")} className="scroll-thin -my-2 flex min-w-0 flex-1 items-center gap-1 overflow-x-auto py-2">
      {TABS.map((t) => (
        <button
          key={t.id}
          type="button"
          onClick={() => onTab(t.id)}
          aria-current={tab === t.id ? "page" : undefined}
          className={cn(
            "relative flex shrink-0 items-center gap-1.5 rounded-full px-3 py-1.5 text-sm font-bold transition-colors active:scale-95",
            tab === t.id ? "text-primary" : "text-muted-foreground hover:text-foreground",
          )}
        >
          {tab === t.id && <motion.span layoutId={`friends-tab-${instanceKey}`} transition={SPRING} className="absolute inset-0 rounded-full bg-primary/15" />}
          <span className="relative">{tr(t.label)}</span>
          <AnimatePresence initial={false}>
            {t.id === "pending" && counts.pending > 0 && (
              <motion.span
                key="badge"
                initial={{ scale: 0 }}
                animate={{ scale: 1 }}
                exit={{ scale: 0 }}
                transition={{ type: "spring", stiffness: 600, damping: 18 }}
                className="relative grid h-5 min-w-5 place-items-center rounded-full bg-primary px-1.5 text-[0.7rem] font-extrabold text-primary-foreground"
              >
                <Count value={counts.pending} max={99} />
              </motion.span>
            )}
          </AnimatePresence>
          {(t.id === "online" || t.id === "all") && counts[t.id] > 0 && (
            <span className="relative text-xs opacity-70">
              <Count value={counts[t.id]} />
            </span>
          )}
        </button>
      ))}
    </nav>
  );
}

/** Ask someone by username. */
function AddFriend({ instanceKey }: { instanceKey: string }) {
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const shake = useAnimationControls();
  const { t } = useI18n();
  // Opening the box is asking to type in it.
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);

  async function submit(e: FormEvent) {
    e.preventDefault();
    const username = cleanUsername(typed);
    if (!username || busy) return;
    setBusy(true);
    setResult(null);
    try {
      const friend = await run(sendFriendRequest(instanceKey, { username }));
      setResult({
        ok: true,
        text: friend?.state === FRIEND ? t("dms-calls.friends.page.nowFriends", { username }) : t("dms-calls.friends.page.requestSent", { username }),
      });
      setTyped("");
    } catch (err) {
      setResult({ ok: false, text: (err as FuwaError).message });
      void shake.start({ x: [0, -8, 7, -5, 3, 0], transition: { duration: 0.4 } });
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="px-3 py-4 sm:px-6">
      <p className="font-extrabold">{t("dms-calls.friends.page.addTitle")}</p>
      <p className="text-sm text-muted-foreground">{t("dms-calls.friends.page.addText")}</p>
      <motion.div animate={shake} className="mt-3 flex items-center gap-2 rounded-2xl border bg-card p-1.5 pl-3 transition focus-within:border-primary/60 focus-within:ring-2 focus-within:ring-primary/20">
        <span className="text-muted-foreground">@</span>
        <input
          ref={input}
          value={typed}
          onChange={(e) => {
            setTyped(e.target.value);
            setResult(null);
          }}
          placeholder={t("dms-calls.friends.page.username")}
          aria-label={t("dms-calls.friends.page.usernameLabel")}
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          maxLength={64}
          className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
        />
        <motion.button
          type="submit"
          whileTap={{ scale: 0.95 }}
          disabled={!cleanUsername(typed) || busy}
          className="flex shrink-0 items-center gap-1.5 rounded-xl bg-primary px-3 py-2 text-sm font-bold text-primary-foreground transition hover:brightness-110 disabled:opacity-50"
        >
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.span key={busy ? "busy" : "idle"} initial={{ scale: 0.4, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} exit={{ scale: 0.4, opacity: 0 }} className="grid">
              {busy ? <LoaderCircleIcon className="size-4 animate-spin" /> : <UserPlusIcon className="size-4" />}
            </motion.span>
          </AnimatePresence>
          {t("dms-calls.friends.page.sendRequest")}
        </motion.button>
      </motion.div>
      <AnimatePresence initial={false}>
        {result && (
          <motion.p
            key={result.text}
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={SPRING}
            role={result.ok ? "status" : "alert"}
            className={cn("mt-2 flex items-center gap-1.5 text-sm font-bold", result.ok ? "text-emerald-600 dark:text-emerald-400" : "text-destructive")}
          >
            {result.ok && (
              <motion.span initial={{ scale: 0, rotate: -45 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 500, damping: 14 }} className="grid">
                <CheckIcon className="size-4" strokeWidth={3} />
              </motion.span>
            )}
            {result.text}
          </motion.p>
        )}
      </AnimatePresence>
    </form>
  );
}

/**
 * The lines of a tab. Every line has the same height (measured from the
 * first one drawn, so it follows the density setting); only those in view
 * are drawn, each placed with a transform that eases when it moves.
 */
function FriendLines({ instanceKey, friends }: { instanceKey: string; friends: Friend[] }) {
  const [height, setHeight] = useState(60);
  const measure = useCallback((el: HTMLElement | null) => {
    const h = el?.offsetHeight ?? 0;
    if (h > 0) setHeight((cur) => (cur === h ? cur : h));
  }, []);
  const scroller = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: 800 });
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const update = () => setView((v) => (v.top === el.scrollTop && v.height === el.clientHeight ? v : { top: el.scrollTop, height: el.clientHeight }));
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const onScroll = useCallback(() => {
    const el = scroller.current;
    if (el) setView({ top: el.scrollTop, height: el.clientHeight });
  }, []);
  const first = Math.max(0, Math.floor(view.top / height) - OVERSCAN);
  const last = Math.min(friends.length, Math.ceil((view.top + view.height) / height) + OVERSCAN);

  return (
    <div ref={scroller} onScroll={onScroll} className="scroll-thin h-full overflow-y-auto px-3 pb-6 sm:px-6">
      <div className="relative" style={{ height: friends.length * height }}>
        <AnimatePresence initial={false} presenceAffectsLayout={false}>
          {friends.slice(first, last).map((friend, n) => (
            <div
              key={friend.user?.id}
              ref={n === 0 ? measure : undefined}
              className="member-line absolute inset-x-0 top-0"
              style={{ transform: `translateY(${(first + n) * height}px)` }}
            >
              <FriendLine instanceKey={instanceKey} friend={friend} />
            </div>
          ))}
        </AnimatePresence>
      </div>
    </div>
  );
}

const FriendLine = memo(function FriendLine({ instanceKey, friend }: { instanceKey: string; friend: Friend }) {
  const user = friend.user;
  const navigate = useNavigate();
  const [busy, setBusy] = useState(false);
  const { t } = useI18n();
  const status = shownStatus(user);
  const line =
    friend.state === INCOMING || friend.state === OUTGOING
      ? pendingLine(t, friend)
      : friend.state === BLOCKED
        ? t("dms-calls.friends.page.blockedLine")
        : status || (friend.online ? t("dms-calls.friends.page.online") : t("dms-calls.friends.page.offline"));

  async function act(action: () => Promise<unknown>, done?: string) {
    if (busy) return;
    setBusy(true);
    try {
      await action();
      if (done) toast(done);
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setBusy(false);
    }
  }
  const id = user?.id ?? "";
  const name = displayName(user);
  const message = () =>
    act(async () => {
      const conversation = await run(openConversation(instanceKey, id));
      void navigate({ to: "/$instance/dm/$conversation", params: { instance: instanceKey, conversation } });
    });

  return (
    <motion.div
      exit={{ opacity: 0, x: 24, transition: { duration: 0.18 } }}
      className="group flex items-center gap-3 border-t border-border/60 py-2 transition-colors first:border-t-0"
    >
      <ProfilePopover instanceKey={instanceKey} user={user} side="right">
        <button type="button" className="flex min-w-0 flex-1 items-center gap-3 rounded-xl px-1 py-1 text-left transition hover:bg-muted/60">
          <span className="relative shrink-0">
            <UserAvatar user={user} className="size-10 text-sm transition duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:scale-105" />
            {friend.state === FRIEND && (
              <motion.span
                aria-label={friend.online ? t("dms-calls.friends.page.online") : t("dms-calls.friends.page.offline")}
                initial={false}
                animate={{ scale: friend.online ? 1 : 0.7 }}
                transition={{ type: "spring", stiffness: 600, damping: 18 }}
                className={cn(
                  "absolute -right-0.5 -bottom-0.5 size-3.5 rounded-full ring-[3px] ring-background transition-colors duration-300",
                  friend.online ? "bg-emerald-500" : "bg-muted-foreground/50",
                )}
              />
            )}
          </span>
          <span className="min-w-0 flex-1">
            <span className="flex min-w-0 items-baseline gap-1.5">
              <span className="truncate font-bold transition-transform duration-300 group-hover:translate-x-0.5">{name}</span>
              <span className="hidden truncate text-xs text-muted-foreground sm:inline">@{user?.username}</span>
            </span>
            <span className="block truncate text-xs text-muted-foreground">{line}</span>
          </span>
        </button>
      </ProfilePopover>
      <LineActions instanceKey={instanceKey} friend={friend} name={name} busy={busy} act={act} onMessage={() => void message()} />
    </motion.div>
  );
});

/** What you can do with someone from their line, by where you stand. */
function LineActions({
  instanceKey,
  friend,
  name,
  busy,
  act,
  onMessage,
}: {
  instanceKey: string;
  friend: Friend;
  name: string;
  busy: boolean;
  act: (action: () => Promise<unknown>, done?: string) => Promise<void>;
  onMessage: () => void;
}) {
  const id = friend.user?.id ?? "";
  const { t } = useI18n();
  const canMessage = useFuwa((s) => {
    const dms = s.instances[instanceKey]?.dms.status;
    return dms === "ready" || dms === "starting";
  });
  return (
    <div className="flex shrink-0 items-center gap-1.5">
      {friend.state === FRIEND && (
        <>
          {canMessage && (
            <RoundButton label={t("dms-calls.friends.page.message", { name })} onClick={onMessage} disabled={busy}>
              <MessageCircleIcon className="size-4 transition-transform duration-300 group-hover/b:-rotate-12" />
            </RoundButton>
          )}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <motion.button
                type="button"
                whileTap={{ scale: 0.88 }}
                aria-label={t("dms-calls.friends.page.more", { name })}
                className="grid size-9 place-items-center rounded-full bg-muted/70 text-muted-foreground transition hover:bg-muted hover:text-foreground data-[state=open]:bg-muted"
              >
                <EllipsisVerticalIcon className="size-4" />
              </motion.button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <DropdownMenuItem onSelect={() => void act(() => run(removeFriend(instanceKey, id)), t("dms-calls.friends.removed", { name }))}>
                <UserMinusIcon /> {t("dms-calls.friends.remove")}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive" onSelect={() => void act(() => run(blockUser(instanceKey, id)), t("dms-calls.friends.page.blocked", { name }))}>
                <BanIcon /> {t("dms-calls.friends.page.block")}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </>
      )}
      {friend.state === INCOMING && (
        <>
          <RoundButton label={t("dms-calls.friends.page.accept", { name })} tone="good" onClick={() => void act(() => run(acceptFriend(instanceKey, id)))} disabled={busy}>
            <CheckIcon className="size-4" strokeWidth={3} />
          </RoundButton>
          <RoundButton label={t("dms-calls.friends.page.decline", { name })} tone="bad" onClick={() => void act(() => run(removeFriend(instanceKey, id)))} disabled={busy}>
            <XIcon className="size-4" />
          </RoundButton>
        </>
      )}
      {friend.state === OUTGOING && (
        <RoundButton label={t("dms-calls.friends.page.cancelTo", { name })} tone="bad" onClick={() => void act(() => run(removeFriend(instanceKey, id)))} disabled={busy}>
          <XIcon className="size-4" />
        </RoundButton>
      )}
      {friend.state === BLOCKED && (
        <motion.button
          type="button"
          whileTap={{ scale: 0.95 }}
          disabled={busy}
          onClick={() => void act(() => run(unblockUser(instanceKey, id)), t("dms-calls.friends.unblocked", { name }))}
          className="group/b flex items-center gap-1.5 rounded-full bg-muted/70 px-3 py-1.5 text-xs font-bold text-muted-foreground transition hover:bg-muted hover:text-foreground disabled:opacity-60"
        >
          <ShieldOffIcon className="size-3.5 transition-transform duration-300 group-hover/b:-rotate-12" /> {t("dms-calls.friends.unblock")}
        </motion.button>
      )}
    </div>
  );
}

function RoundButton({
  label,
  tone,
  onClick,
  disabled,
  children,
}: {
  label: string;
  tone?: "good" | "bad";
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  return (
    <motion.button
      type="button"
      whileHover={{ scale: 1.08 }}
      whileTap={{ scale: 0.88 }}
      transition={{ type: "spring", stiffness: 600, damping: 20 }}
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      title={label}
      className={cn(
        "group/b grid size-9 place-items-center rounded-full bg-muted/70 text-muted-foreground transition-colors disabled:opacity-60",
        tone === "good" ? "hover:bg-emerald-500/15 hover:text-emerald-600 dark:hover:text-emerald-400" : tone === "bad" ? "hover:bg-destructive/10 hover:text-destructive" : "hover:bg-muted hover:text-foreground",
      )}
    >
      {children}
    </motion.button>
  );
}

function Loading() {
  return (
    <div className="flex flex-col gap-3 px-4 pt-2 sm:px-7">
      {[0, 1, 2].map((n) => (
        <div key={n} className="flex items-center gap-3">
          <span className="shimmer size-10 rounded-full" />
          <span className="flex flex-1 flex-col gap-1.5">
            <span className="shimmer h-3 w-1/3 rounded" />
            <span className="shimmer h-2.5 w-1/5 rounded" />
          </span>
        </div>
      ))}
    </div>
  );
}

function TabEmpty({ tab, searching, onAdd }: { tab: FriendsTab; searching: boolean; onAdd: () => void }) {
  const { t } = useI18n();
  if (searching) return <Empty icon={<SearchIcon className="size-7" />} title={t("dms-calls.friends.page.noMatchTitle")} text={t("dms-calls.friends.page.noMatchText")} />;
  if (tab === "blocked") return <Empty icon={<BanIcon className="size-7" />} title={t("dms-calls.friends.page.noBlockedTitle")} text={t("dms-calls.friends.page.noBlockedText")} />;
  if (tab === "pending")
    return <Empty icon={<HeartHandshakeIcon className="size-7" />} title={t("dms-calls.friends.page.noPendingTitle")} text={t("dms-calls.friends.page.noPendingText")} />;
  return (
    <Empty
      icon={<UsersIcon className="size-7" />}
      title={tab === "online" ? t("dms-calls.friends.page.noOnlineTitle") : t("dms-calls.friends.page.noFriendsTitle")}
      text={tab === "online" ? t("dms-calls.friends.page.noOnlineText") : t("dms-calls.friends.page.noFriendsText")}
      action={
        tab === "all" ? (
          <motion.button
            type="button"
            whileTap={{ scale: 0.95 }}
            onClick={onAdd}
            className="mt-2 flex items-center gap-1.5 rounded-full bg-primary px-4 py-2 text-sm font-bold text-primary-foreground transition hover:brightness-110"
          >
            <UserPlusIcon className="size-4" /> {t("dms-calls.friends.add")}
          </motion.button>
        ) : null
      }
    />
  );
}

function Empty({ icon, title, text, action }: { icon: React.ReactNode; title: string; text: string; action?: React.ReactNode }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={SPRING}
      className="mx-auto flex max-w-sm flex-col items-center px-6 pt-16 text-center"
    >
      <span className="float grid size-16 place-items-center rounded-3xl bg-primary/15 text-primary">{icon}</span>
      <p className="mt-4 font-extrabold">{title}</p>
      <p className="mt-1 text-sm text-muted-foreground">{text}</p>
      {action}
    </motion.div>
  );
}
