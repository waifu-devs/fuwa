import { ArchiveIcon, ChevronLeftIcon, FilterXIcon, PlusIcon, SearchIcon, Settings2Icon, TagIcon, UserIcon } from "lucide-react";
import { AnimatePresence, LayoutGroup, m as motion } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState, type ComponentProps, type ReactNode } from "react";
import { Permission, type BoardColumn, type Card, type Channel, type Message, type User } from "@/gen/fuwa/v1/types_pb";
import { createCard, listArchived, moveCard, updateCard, useBoard, type BoardView as Board } from "@/fuwa/board";
import { useAccess, useRoles } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { ChannelTitle, ConnectionBadge, useFocusAndTitle } from "@/components/chat/ChannelView";
import { ServerLookProvider, type ServerLook } from "@/components/chat/mentions";
import { NotificationBell } from "@/components/chat/NotificationBell";
import { InlineMarkdown } from "@/components/Markdown";
import { Count } from "@/components/motion";
import { useLayout } from "@/components/Shell";
import { DropdownMenu, DropdownMenuContent, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { useI18n } from "@/i18n/react";
import { cardsIn, colorCss, ms, overLimit, shows, step, TITLE_MAX, type BoardFilter } from "@/lib/board";
import { SPRING } from "@/lib/motion";
import { hasIn } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";
import { useCardDrag } from "@/hooks/use-card-drag";
import { BoardCard, LabelChip } from "./BoardCard";
import { BoardSettingsDialog } from "./BoardSettingsDialog";
import { CardPanel } from "./CardPanel";

const EMPTY: never[] = [];

/**
 * A board channel (docs/boards.md): columns of cards, each card a message
 * whose comments are its thread. Cards drag between columns (or move with
 * Alt+arrows); one opens beside the board, or over it on a narrow screen.
 */
export function BoardView({ instanceKey, serverId, channel }: { instanceKey: string; serverId: string; channel: Channel }) {
  const { t } = useI18n();
  const { compact, setNavOpen } = useLayout();
  const docked = useMediaQuery("(min-width: 1024px)");
  useFocusAndTitle(instanceKey, serverId, channel);
  const board = useBoard(instanceKey, serverId, channel.id);
  const access = useAccess(instanceKey, serverId);
  const me = useFuwa((s) => s.instances[instanceKey]?.me?.id ?? "");
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  const canManage = hasIn(access, channel.id, Permission.MANAGE_CARDS);
  const canAdd = canManage && hasIn(access, channel.id, Permission.SEND_MESSAGES);
  const canSetUp = hasIn(access, channel.id, Permission.MANAGE_CHANNELS);
  const look = useLook(instanceKey, serverId);

  const [filter, setFilter] = useState<BoardFilter>({});
  const [openId, setOpenId] = useState<string | null>(null);
  const [archivedOpen, setArchivedOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const archive = useArchive(instanceKey, serverId, channel.id, archivedOpen);
  // A clock for due dates, a minute at a time.
  const now = useMinute();

  const opened = openId ? (board.cards.find((c) => c.id === openId) ?? archive.cards.find((c) => c.id === openId)) : undefined;
  const openedBody = opened ? (board.bodies[opened.id] ?? archive.bodies[opened.id]) : undefined;
  // Closed when its card is deleted.
  useEffect(() => {
    if (openId && board.loaded && !opened) setOpenId(null);
  }, [openId, board.loaded, opened]);

  const scroller = useRef<HTMLDivElement>(null);
  const onMove = useCallback(
    (cardId: string, columnId: string, beforeId: string) => void moveCard(instanceKey, serverId, channel.id, cardId, columnId, beforeId).catch((err: Error) => toast(err.message)),
    [instanceKey, serverId, channel.id],
  );
  useCardDrag({ container: scroller, enabled: canManage, onMove });
  const boardRef = useRef(board);
  useEffect(() => {
    boardRef.current = board;
  }, [board]);
  const onStep = useCallback(
    (cardId: string, way: "up" | "down" | "left" | "right") => {
      const to = step(boardRef.current, cardId, way);
      if (!to) return;
      onMove(cardId, to.columnId, to.beforeId);
      // Keep the keyboard on the card as it moves.
      requestAnimationFrame(() => scroller.current?.querySelector<HTMLElement>(`[data-card][data-id="${cardId}"]`)?.focus());
    },
    [onMove],
  );

  const filtering = !!(filter.assignee || filter.labelId || filter.query);
  const side = opened ? (
    <CardPanel
      key={opened.id}
      instanceKey={instanceKey}
      serverId={serverId}
      channel={channel}
      card={opened}
      body={openedBody}
      columns={board.columns}
      labels={board.labels}
      onChanged={archive.changed}
      onClose={() => setOpenId(null)}
    />
  ) : archivedOpen ? (
    <ArchivedList instanceKey={instanceKey} serverId={serverId} channel={channel} board={board} archive={archive} canManage={canManage} onOpen={setOpenId} onClose={() => setArchivedOpen(false)} />
  ) : null;

  return (
    <ServerLookProvider value={look}>
      <div className="relative flex h-full min-h-0">
        <div className="flex min-w-0 flex-1 flex-col">
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
            <ChannelTitle channel={channel} />
            {channel.topic && (
              <>
                <span className="hidden h-5 w-px bg-border md:block" />
                <InlineMarkdown className="hidden min-w-0 truncate text-sm text-muted-foreground md:block">{channel.topic}</InlineMarkdown>
              </>
            )}
            <span className="flex-1" />
            <ConnectionBadge instanceKey={instanceKey} />
            <NotificationBell instanceKey={instanceKey} serverId={serverId} channel={channel} />
            <HeaderButton label={t("chattools.board.archived")} active={archivedOpen && !opened} onClick={() => (setOpenId(null), setArchivedOpen(!archivedOpen || !!opened))}>
              <ArchiveIcon />
            </HeaderButton>
            {canSetUp && (
              <HeaderButton label={t("chattools.board.settings")} onClick={() => setSettingsOpen(true)}>
                <Settings2Icon />
              </HeaderButton>
            )}
          </header>
          <Filters board={board} me={me} filter={filter} onFilter={setFilter} />
          {board.error && !board.loaded ? (
            <p className="m-4 rounded-xl bg-destructive/10 p-3 text-sm font-bold text-destructive">{board.error}</p>
          ) : (
            <div ref={scroller} className="scroll-thin flex min-h-0 flex-1 items-start gap-3 overflow-x-auto p-3 sm:p-4">
              <LayoutGroup id={`board-${channel.id}`}>
                {!board.loaded
                  ? [0, 1, 2].map((i) => <div key={i} className="shimmer h-64 w-72 shrink-0 rounded-2xl opacity-40" />)
                  : board.columns.map((column) => (
                      <Column
                        key={column.id}
                        instanceKey={instanceKey}
                        serverId={serverId}
                        channelId={channel.id}
                        column={column}
                        board={board}
                        cards={cardsIn(board, column.id).filter((c) => !filtering || shows(c, board.bodies[c.id], filter))}
                        users={users}
                        now={now}
                        openId={openId}
                        canAdd={canAdd && !filtering}
                        onOpen={setOpenId}
                        onStep={canManage ? onStep : undefined}
                      />
                    ))}
              </LayoutGroup>
            </div>
          )}
        </div>
        <AnimatePresence initial={false} mode="popLayout">
          {side && docked && (
            <motion.aside
              key={opened ? "card" : "archived"}
              initial={{ x: 32, opacity: 0 }}
              animate={{ x: 0, opacity: 1 }}
              exit={{ x: 32, opacity: 0 }}
              transition={{ type: "spring", stiffness: 400, damping: 40 }}
              className="surface-side h-full w-[26rem] shrink-0 overflow-hidden border-l"
            >
              {side}
            </motion.aside>
          )}
          {side && !docked && (
            <motion.aside
              key={opened ? "card-sheet" : "archived-sheet"}
              initial={{ x: "100%" }}
              animate={{ x: 0 }}
              exit={{ x: "100%" }}
              transition={{ type: "spring", stiffness: 420, damping: 40 }}
              className="surface-side absolute inset-0 z-30 flex flex-col shadow-2xl"
            >
              {side}
            </motion.aside>
          )}
        </AnimatePresence>
        {canSetUp && (
          <BoardSettingsDialog
            open={settingsOpen}
            onOpenChange={setSettingsOpen}
            instanceKey={instanceKey}
            serverId={serverId}
            channelId={channel.id}
            columns={board.columns}
            labels={board.labels}
            counts={Object.fromEntries(board.columns.map((c) => [c.id, cardsIn(board, c.id).length]))}
          />
        )}
      </div>
    </ServerLookProvider>
  );
}

/** What messages on the board need to draw names, mentions and emoji. */
function useLook(instanceKey: string, serverId: string): ServerLook {
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? EMPTY);
  const emojis = useFuwa((s) => s.instances[instanceKey]?.emojis[serverId] ?? EMPTY);
  const me = useFuwa((s) => s.instances[instanceKey]?.me ?? undefined);
  const ownerId = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.ownerId ?? "");
  const roles = useRoles(instanceKey, serverId);
  return useMemo<ServerLook>(() => {
    const myRoles = members.find((m) => m.user?.id === me?.id)?.roleIds ?? [];
    return { instanceKey, ownerId, roles, members, emojis, me: me ? { id: me.id, username: me.username, roleIds: myRoles } : undefined };
  }, [instanceKey, ownerId, roles, members, emojis, me]);
}

function useMinute() {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(timer);
  }, []);
  return now;
}

type Archive = {
  cards: Card[];
  bodies: Record<string, Message>;
  more: boolean;
  loading: boolean;
  loadMore: () => void;
  /** A card changed from its panel: archived ones stay listed, restored ones leave. */
  changed: (card: Card) => void;
};

/** The board's archived cards, read while their list is open. */
function useArchive(instanceKey: string, serverId: string, channelId: string, open: boolean): Archive {
  const [cards, setCards] = useState<Card[]>([]);
  const [bodies, setBodies] = useState<Record<string, Message>>({});
  const [more, setMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const page = useCallback(
    (after: string) => {
      setLoading(true);
      listArchived(instanceKey, serverId, channelId, after)
        .then((res) => {
          setCards((had) => (after ? [...had, ...res.cards] : res.cards));
          setBodies((had) => (after ? { ...had, ...res.bodies } : res.bodies));
          setMore(res.hasMore);
        })
        .catch((err: Error) => toast(err.message))
        .finally(() => setLoading(false));
    },
    [instanceKey, serverId, channelId],
  );
  useEffect(() => {
    if (open) page("");
  }, [open, page]);
  const changed = useCallback((card: Card) => {
    setCards((had) => (ms(card.archivedAt) ? (had.some((c) => c.id === card.id) ? had.map((c) => (c.id === card.id ? card : c)) : [card, ...had]) : had.filter((c) => c.id !== card.id)));
  }, []);
  return { cards, bodies, more, loading, loadMore: () => page(cards.at(-1)?.id ?? ""), changed };
}

function HeaderButton({ label, active, onClick, children }: { label: string; active?: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <motion.button
      type="button"
      aria-label={label}
      aria-pressed={active}
      title={label}
      onClick={onClick}
      whileTap={{ scale: 0.85 }}
      className={cn("grid size-9 place-items-center rounded-full text-muted-foreground transition-colors hover:bg-muted [&_svg]:size-5", active && "bg-primary/15 text-primary")}
    >
      {children}
    </motion.button>
  );
}

/** Narrowing the board: words, your cards, a label. */
function Filters({ board, me, filter, onFilter }: { board: Board; me: string; filter: BoardFilter; onFilter: (f: BoardFilter) => void }) {
  const { t } = useI18n();
  const label = board.labels.find((l) => l.id === filter.labelId);
  const any = !!(filter.assignee || filter.labelId || filter.query);
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2 sm:px-4">
      <label className="flex h-8 min-w-40 flex-1 items-center gap-2 rounded-full border bg-card px-3 text-sm focus-within:border-primary/60 sm:max-w-64 sm:flex-none">
        <SearchIcon className="size-3.5 shrink-0 text-muted-foreground" />
        <input
          value={filter.query ?? ""}
          onChange={(e) => onFilter({ ...filter, query: e.target.value })}
          placeholder={t("chattools.board.filter")}
          aria-label={t("chattools.board.filter")}
          className="min-w-0 flex-1 bg-transparent outline-none"
        />
      </label>
      <Chip active={filter.assignee === me} onClick={() => onFilter({ ...filter, assignee: filter.assignee === me ? undefined : me })}>
        <UserIcon className="size-3.5" /> {t("chattools.board.mine")}
      </Chip>
      {board.labels.length > 0 && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Chip active={!!label}>
              <TagIcon className="size-3.5" /> {label ? <LabelChip label={label} /> : t("chattools.board.anyLabel")}
            </Chip>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-52">
            <DropdownMenuRadioGroup value={filter.labelId ?? ""} onValueChange={(id) => onFilter({ ...filter, labelId: id || undefined })}>
              <DropdownMenuRadioItem value="">{t("chattools.board.anyLabel")}</DropdownMenuRadioItem>
              {board.labels.map((l) => (
                <DropdownMenuRadioItem key={l.id} value={l.id}>
                  <LabelChip label={l} />
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      )}
      <AnimatePresence>
        {any && (
          <motion.button
            type="button"
            initial={{ opacity: 0, scale: 0.8 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.8 }}
            transition={SPRING}
            onClick={() => onFilter({})}
            className="flex h-8 items-center gap-1.5 rounded-full px-3 text-xs font-bold text-muted-foreground hover:bg-muted hover:text-foreground"
          >
            <FilterXIcon className="size-3.5" /> {t("chattools.board.clearFilters")}
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}

/** A filter's button; it takes a menu's trigger props too. */
function Chip({ active, children, ...rest }: { active: boolean; children: ReactNode } & ComponentProps<"button">) {
  return (
    <button
      type="button"
      aria-pressed={active}
      {...rest}
      className={cn(
        "flex h-8 items-center gap-1.5 rounded-full border px-3 text-xs font-bold transition-[color,background-color,border-color,scale] active:scale-95",
        active ? "border-primary/60 bg-primary/15 text-primary" : "bg-card text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

function Column({
  instanceKey,
  serverId,
  channelId,
  column,
  board,
  cards,
  users,
  now,
  openId,
  canAdd,
  onOpen,
  onStep,
}: {
  instanceKey: string;
  serverId: string;
  channelId: string;
  column: BoardColumn;
  board: Board;
  cards: Card[];
  users: Record<string, User> | undefined;
  now: number;
  openId: string | null;
  canAdd: boolean;
  onOpen: (id: string) => void;
  onStep?: (id: string, way: "up" | "down" | "left" | "right") => void;
}) {
  const { t } = useI18n();
  const [adding, setAdding] = useState(false);
  const color = colorCss(column.color);
  const over = overLimit(board, column);
  const total = cardsIn(board, column.id).length;
  return (
    <motion.section
      layout="position"
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={SPRING}
      data-column
      data-id={column.id}
      aria-label={column.name}
      className="flex max-h-full w-72 shrink-0 flex-col rounded-2xl border bg-muted/40"
    >
      <header className="flex items-center gap-2 px-3 pt-3 pb-2">
        <span className="size-2.5 shrink-0 rounded-full bg-muted-foreground" style={color ? { background: color } : undefined} />
        <h2 className="min-w-0 flex-1 truncate text-sm font-extrabold">{column.name}</h2>
        <span
          className={cn("rounded-full px-2 py-px text-xs font-bold text-muted-foreground tabular-nums", over && "bg-destructive/15 text-destructive")}
          title={column.cardLimit ? t("chattools.board.limit", { count: column.cardLimit }) : undefined}
        >
          <Count value={total} />
          {column.cardLimit > 0 && ` / ${column.cardLimit}`}
        </span>
        {canAdd && (
          <button
            type="button"
            aria-label={t("chattools.board.addCardTo", { column: column.name })}
            onClick={() => setAdding(true)}
            className="grid size-7 place-items-center rounded-full text-muted-foreground transition hover:rotate-90 hover:bg-muted hover:text-foreground"
          >
            <PlusIcon className="size-4" />
          </button>
        )}
      </header>
      <div data-cards className="scroll-thin flex min-h-12 flex-col gap-2 overflow-y-auto px-2 pb-2">
        <AnimatePresence initial={false} mode="popLayout">
          {cards.map((card) => (
            <BoardCard
              key={card.id}
              card={card}
              body={board.bodies[card.id]}
              column={column}
              labels={board.labels.filter((l) => card.labelIds.includes(l.id))}
              people={card.assigneeIds.map((id) => users?.[id])}
              now={now}
              selected={openId === card.id}
              onOpen={onOpen}
              onStep={onStep}
            />
          ))}
        </AnimatePresence>
        {!cards.length && !adding && <p className="px-2 py-3 text-center text-xs text-muted-foreground">{t("chattools.board.emptyColumn")}</p>}
      </div>
      {canAdd && (
        <div className="px-2 pb-2">
          <AnimatePresence mode="wait" initial={false}>
            {adding ? (
              <QuickAdd key="add" instanceKey={instanceKey} serverId={serverId} channelId={channelId} columnId={column.id} onDone={() => setAdding(false)} />
            ) : (
              <motion.button
                key="button"
                type="button"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                onClick={() => setAdding(true)}
                className="flex w-full items-center gap-1.5 rounded-xl px-2 py-1.5 text-sm font-bold text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
              >
                <PlusIcon className="size-4" /> {t("chattools.board.addCard")}
              </motion.button>
            )}
          </AnimatePresence>
        </div>
      )}
    </motion.section>
  );
}

/** A new card by its title, at the column's bottom; Enter adds it and starts the next. */
function QuickAdd({ instanceKey, serverId, channelId, columnId, onDone }: { instanceKey: string; serverId: string; channelId: string; columnId: string; onDone: () => void }) {
  const { t } = useI18n();
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const add = async () => {
    const text = title.trim();
    if (!text || busy) return;
    setBusy(true);
    try {
      await createCard(instanceKey, serverId, channelId, { columnId, title: text, content: "", attachments: [], emojis: [] });
      setTitle("");
    } catch (err) {
      toast((err as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <motion.div initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 6 }} transition={SPRING} className="flex flex-col gap-1.5">
      <textarea
        autoFocus
        value={title}
        maxLength={TITLE_MAX}
        rows={2}
        disabled={busy}
        placeholder={t("chattools.board.newCardPlaceholder")}
        aria-label={t("chattools.board.newCardPlaceholder")}
        onChange={(e) => setTitle(e.target.value.replace(/\n/g, " "))}
        onBlur={() => !title.trim() && onDone()}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            void add();
          }
          if (e.key === "Escape") onDone();
        }}
        className="w-full resize-none rounded-xl border bg-card px-2.5 py-2 text-sm font-bold shadow-sm outline-none focus:border-primary/60"
      />
      <p className="px-1 text-[0.68rem] text-muted-foreground">{t("chattools.board.newCardHint")}</p>
    </motion.div>
  );
}

/** The board's archived cards, latest first, to open or put back. */
function ArchivedList({
  instanceKey,
  serverId,
  channel,
  board,
  archive,
  canManage,
  onOpen,
  onClose,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  board: Board;
  archive: Archive;
  canManage: boolean;
  onOpen: (id: string) => void;
  onClose: () => void;
}) {
  const { t, date } = useI18n();
  const restore = (card: Card) =>
    updateCard(instanceKey, serverId, channel.id, card, { archived: false })
      .then((c) => c && archive.changed(c))
      .catch((err: Error) => toast(err.message));
  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-14 shrink-0 items-center gap-2 border-b px-4">
        <ArchiveIcon className="size-4 text-muted-foreground" />
        <h2 className="flex-1 text-sm font-extrabold">{t("chattools.board.archived")}</h2>
        <button type="button" onClick={onClose} className="rounded-full px-3 py-1 text-xs font-bold text-muted-foreground hover:bg-muted">
          {t("chattools.board.close")}
        </button>
      </header>
      <ul className="scroll-thin flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto p-3">
        <AnimatePresence initial={false} mode="popLayout">
          {archive.cards.map((card) => {
            const column = board.columns.find((c) => c.id === card.columnId);
            return (
              <motion.li
                key={card.id}
                layout
                initial={{ opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, x: -24 }}
                transition={SPRING}
                className="flex items-center gap-2 rounded-xl border bg-card p-2.5"
              >
                <button type="button" onClick={() => onOpen(card.id)} className="min-w-0 flex-1 text-left">
                  <p className="truncate text-sm font-bold">{card.title}</p>
                  <p className="truncate text-xs text-muted-foreground">
                    {column?.name ?? "?"} · {date(new Date(ms(card.archivedAt)), { month: "short", day: "numeric" })}
                  </p>
                </button>
                {canManage && (
                  <button type="button" onClick={() => void restore(card)} className="shrink-0 rounded-lg px-2 py-1 text-xs font-bold text-primary hover:bg-primary/10">
                    {t("chattools.board.restore")}
                  </button>
                )}
              </motion.li>
            );
          })}
        </AnimatePresence>
        {!archive.cards.length && !archive.loading && <p className="p-4 text-center text-sm text-muted-foreground">{t("chattools.board.noArchived")}</p>}
        {archive.loading && <li className="shimmer h-12 rounded-xl opacity-40" />}
        {archive.more && !archive.loading && (
          <button type="button" onClick={archive.loadMore} className="rounded-xl py-2 text-sm font-bold text-primary hover:bg-primary/10">
            {t("chattools.board.more")}
          </button>
        )}
      </ul>
    </div>
  );
}
