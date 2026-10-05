import { useNavigate } from "@tanstack/react-router";
import { BellOffIcon, BotIcon, ChartColumnIcon, ChevronRightIcon, EllipsisIcon, EyeOffIcon, MessagesSquareIcon, Trash2Icon, MonitorUpIcon, PowerOffIcon, RadioTowerIcon, RotateCcwIcon, TrophyIcon, VideoIcon, Volume2Icon, WebhookIcon } from "lucide-react";
import { AnimatePresence, LayoutGroup, m as motion, useReducedMotion } from "motion/react";
import { useEffect, useMemo, type ReactNode } from "react";
import { joinCall } from "@/calls/engine";
import { useCalls } from "@/calls/state";
import { Count } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { useLayout } from "@/components/Shell";
import { UserAvatar } from "@/components/Icons";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuLabel, DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { endLiveTile, loadFollowed, run, updateServer } from "@/fuwa/actions";
import { useAccess } from "@/fuwa/hooks";
import { usePresenceSettings } from "@/fuwa/presence";
import { notificationKey, useFuwa } from "@/fuwa/store";
import { PresenceStatus } from "@/gen/fuwa/v1/presence_pb";
import { Permission } from "@/gen/fuwa/v1/types_pb";
import { useI18n } from "@/i18n/react";
import { instanceHas } from "@/lib/compat";
import { formatTime } from "@/lib/format";
import { hasIn } from "@/lib/permissions";
import { BIG_SERVER, collectTiles, kindNumbers, pickTiles, POLL_SOON_MS, removable, shownKinds, TILE_KINDS, type Tile, type TileKind } from "@/lib/live-tiles";
import { hideTile, setServerQuiet, setTilesOn, useLiveTilesLocal } from "@/lib/live-tiles-store";
import { toast } from "@/lib/ui";
import { isMuted, useNow } from "@/lib/notifications";
import { usePrefs } from "@/lib/prefs";
import { requestThread } from "@/lib/threads";
import { cn } from "@/lib/utils";

/**
 * Live tiles at the top of a server's channel list (see lib/live-tiles.ts),
 * on instances that have them. Quiet by design: at most three, only the kinds
 * the server shows, none while you're on Do not disturb or have the server
 * muted, and each one hides with a click.
 */

const KIND: Record<TileKind, { icon: typeof Volume2Icon; tint: string }> = {
  voice: { icon: Volume2Icon, tint: "from-emerald-500 to-teal-500" },
  poll: { icon: ChartColumnIcon, tint: "from-amber-500 to-orange-500" },
  thread: { icon: MessagesSquareIcon, tint: "from-sky-500 to-indigo-500" },
  shared: { icon: RadioTowerIcon, tint: "from-violet-500 to-purple-500" },
  app: { icon: TrophyIcon, tint: "from-lime-500 to-emerald-600" },
};

const NONE: never[] = [];

/** Whether the instance has live tiles at all (older ones don't). */
const useTilesHere = (instanceKey: string) => useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "live-tiles"));

/** The kinds a server shows, as its setting says. */
function useServerKinds(instanceKey: string, serverId: string) {
  const setting = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.liveTiles);
  const members = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.memberCount ?? 0n);
  return useMemo(() => ({ customized: !!setting?.customized, kinds: shownKinds(setting, members) }), [setting, members]);
}

export function LiveTiles({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { on, hidden, quiet } = useLiveTilesLocal();
  const here = useTilesHere(instanceKey);
  const { kinds } = useServerKinds(instanceKey, serverId);
  if (!here || !on || quiet.has(serverId) || kinds.size === 0) return null;
  return <Strip instanceKey={instanceKey} serverId={serverId} hidden={hidden} kinds={kinds} />;
}

function Strip({ instanceKey, serverId, hidden, kinds }: { instanceKey: string; serverId: string; hidden: ReadonlySet<string>; kinds: ReadonlySet<TileKind> }) {
  const { t } = useI18n();
  // Countdowns are in minutes and apps' tiles run out by the minute; rooms, threads and apps' changes move with the store.
  const now = useNow(15_000);
  const apps = useFuwa((s) => s.instances[instanceKey]?.liveTiles[serverId] ?? NONE);
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[serverId]);
  const voice = useFuwa((s) => s.instances[instanceKey]?.voice[serverId] ?? NONE);
  const messages = useFuwa((s) => s.instances[instanceKey]?.messages);
  const threadParents = useFuwa((s) => s.instances[instanceKey]?.threadParents);
  const followed = useFuwa((s) => s.instances[instanceKey]?.followed[serverId]);
  const threadUnread = useFuwa((s) => s.instances[instanceKey]?.threadUnread);
  const unread = useFuwa((s) => s.instances[instanceKey]?.unread);
  const notifications = useFuwa((s) => s.instances[instanceKey]?.notifications);
  const inChannel = useCalls((s) => (s.call?.target.kind === "voice" && s.call.target.serverId === serverId ? s.call.target.channelId : null));
  const dnd = usePresenceSettings(instanceKey)?.status === PresenceStatus.DO_NOT_DISTURB;
  // Followed threads count new replies only once the app knows which they are (one small call per server).
  useEffect(() => {
    run(loadFollowed(instanceKey, serverId)).catch(() => {});
  }, [instanceKey, serverId]);

  const tiles = useMemo(() => {
    if (!channels || !messages || !threadParents || !threadUnread || !unread || !notifications) return [];
    const real = collectTiles(
      {
        channels,
        voice,
        messages,
        threadParents,
        followed,
        threadUnread,
        unread,
        inChannel,
        muted: (id) => isMuted(notifications[notificationKey(serverId, id)], now),
        apps,
      },
      now,
    );
    return pickTiles(real, hidden, now, kinds);
  }, [channels, voice, messages, threadParents, followed, threadUnread, unread, notifications, inChannel, serverId, now, apps, hidden, kinds]);

  const serverMuted = isMuted(notifications?.[notificationKey(serverId)], now);
  const shown = dnd || serverMuted ? [] : tiles;

  return (
    <AnimatePresence initial={false}>
      {shown.length > 0 && (
        <motion.section
          key="strip"
          layout="position"
          aria-label={t("tiles.strip.label")}
          data-testid="live-tiles"
          initial={{ opacity: 0, y: -8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -8, transition: { duration: 0.16 } }}
          transition={SPRING}
          className="mb-3"
        >
          <h2 className="mb-1.5 flex items-center gap-1.5 px-1.5 text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">
            <LiveDot />
            {t("tiles.strip.label")}
          </h2>
          <ul className="flex flex-col gap-1.5">
            <AnimatePresence mode="popLayout" initial={false}>
              {shown.map((tile, n) => (
                <TileCard key={tile.id} tile={tile} index={n} instanceKey={instanceKey} serverId={serverId} now={now} />
              ))}
            </AnimatePresence>
          </ul>
        </motion.section>
      )}
    </AnimatePresence>
  );
}

/** The "live" dot: a soft ring breathes out of it, unless motion is turned down. */
function LiveDot() {
  const reduce = useQuiet();
  return (
    <span className="relative grid size-2 place-items-center" aria-hidden>
      {!reduce && (
        <motion.span
          className="absolute inset-0 rounded-full bg-rose-500"
          animate={{ scale: [1, 2.6], opacity: [0.55, 0] }}
          transition={{ duration: 1.8, repeat: Infinity, ease: "easeOut" }}
        />
      )}
      <span className="size-2 rounded-full bg-rose-500" />
    </span>
  );
}

/** Motion turned down, by the system or the app's own setting. */
function useQuiet() {
  const system = useReducedMotion();
  const setting = usePrefs((p) => p.reduceMotion);
  return setting === "always" || (setting === "system" && !!system);
}

function TileCard({ tile, index, instanceKey, serverId, now }: { tile: Tile; index: number; instanceKey: string; serverId: string; now: number }) {
  const { t } = useI18n();
  const navigate = useNavigate();
  const { compact, setNavOpen } = useLayout();
  const { icon: Icon, tint } = KIND[tile.kind];

  const open = () => {
    navigate({ to: "/$instance/$server/$channel", params: { instance: instanceKey, server: serverId, channel: tile.channelId } });
    if (tile.kind === "thread") requestThread(tile.channelId, tile.threadId);
    if (tile.kind === "voice") void joinCall({ kind: "voice", instance: instanceKey, serverId, channelId: tile.channelId });
    if (compact) setNavOpen(false);
  };

  const body = describe(tile, now, t, instanceKey);

  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, y: -10, scale: 0.96 }}
      animate={{ opacity: 1, y: 0, scale: 1, transition: { ...SPRING, delay: index * 0.05 } }}
      exit={{ opacity: 0, scale: 0.94, transition: { duration: 0.16 } }}
      transition={SPRING}
      whileHover={{ y: -1 }}
      data-tile={tile.kind}
      className="group relative overflow-hidden rounded-xl border bg-card/80 shadow-xs"
    >
      {tile.kind === "app" && tile.live && <Sweep />}
      <div className="flex items-start gap-2 p-2">
        <button type="button" onClick={open} className="flex min-w-0 flex-1 items-start gap-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/60 rounded-lg">
          <span className={cn("grid size-8 shrink-0 place-items-center rounded-lg bg-gradient-to-br text-white shadow-sm", tint)}>
            <Icon className="size-4 transition-transform duration-300 group-hover:scale-110 group-hover:-rotate-6" />
          </span>
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm leading-5 font-bold">{body.title}</span>
            <span className="block truncate text-xs leading-4 text-muted-foreground">{body.line}</span>
          </span>
        </button>
        <TileMenu tile={tile} instanceKey={instanceKey} serverId={serverId} />
      </div>
      {body.rows}
      <div className="flex items-center gap-2 px-2 pb-2">
        <span className="flex min-w-0 flex-1 items-center gap-1.5 text-xs text-muted-foreground">{body.extra}</span>
        <motion.button
          type="button"
          onClick={open}
          whileTap={{ scale: 0.94 }}
          className="inline-flex h-6 shrink-0 items-center gap-0.5 rounded-md bg-primary/12 px-2 text-xs font-bold text-primary transition-colors hover:bg-primary/20"
        >
          {body.action}
          <ChevronRightIcon className="size-3.5 transition-transform group-hover:translate-x-0.5" />
        </motion.button>
      </div>
      {body.left !== null && <Countdown left={body.left} tint={tint} />}
    </motion.li>
  );
}

type Body = { title: string; line: ReactNode; extra: ReactNode; action: string; left: number | null; rows?: ReactNode };

/** A tile's words, and how much of its window is left (for the bar along its bottom). */
function describe(tile: Tile, now: number, t: ReturnType<typeof useI18n>["t"], instanceKey: string): Body {
  switch (tile.kind) {
    case "voice":
      return {
        title: t("tiles.voice.title", { count: tile.userIds.length, channel: tile.channelName }),
        line: tile.screen ? t("tiles.voice.screen") : tile.video ? t("tiles.voice.video") : t("tiles.voice.since", { time: formatTime(new Date(tile.since)) }),
        extra: <Faces instanceKey={instanceKey} userIds={tile.userIds} flags={<VoiceHints video={tile.video} screen={tile.screen} />} />,
        action: t("tiles.voice.join"),
        left: null,
      };
    case "poll": {
      const left = tile.endsAt === null ? null : Math.max(0, tile.endsAt - now);
      const minutes = left === null ? 0 : Math.max(1, Math.ceil(left / 60_000));
      return {
        title: t("tiles.poll.title", { channel: tile.channelName }),
        line: tile.question,
        extra: (
          <span className="truncate">
            {left === null ? t("tiles.poll.noEnd") : minutes < 60 ? t("tiles.poll.closesMinutes", { count: minutes }) : t("tiles.poll.closesHours", { count: Math.round(minutes / 60) })}
            {" · "}
            {t("tiles.poll.voters", { count: tile.voters })}
          </span>
        ),
        action: t("tiles.poll.vote"),
        left: left === null ? null : left / POLL_SOON_MS,
      };
    }
    case "thread":
      return {
        title: tile.title,
        line: t("tiles.thread.title"),
        extra: <Faces instanceKey={instanceKey} userIds={tile.userIds} flags={<span className="truncate">{t("tiles.thread.unread", { count: tile.unread })}</span>} />,
        action: t("tiles.thread.open"),
        left: null,
      };
    case "shared":
      return {
        title: t("tiles.shared.title", { channel: tile.channelName }),
        line: t("tiles.shared.text", { count: tile.unread, servers: tile.servers }),
        extra: (
          <span className="tabular-nums">
            <Count value={tile.unread} max={99} />
          </span>
        ),
        action: t("tiles.shared.open"),
        left: null,
      };
    case "app": {
      const From = tile.webhook ? WebhookIcon : BotIcon;
      const status = tile.live ? [t("tiles.app.live"), tile.status].filter(Boolean).join(" · ") : tile.status;
      return {
        title: tile.title,
        line: (
          <span className="inline-flex min-w-0 items-center gap-1">
            <From className="size-3 shrink-0" />
            <span className="truncate">{t("tiles.app.by", { app: tile.app })}</span>
          </span>
        ),
        rows: tile.rows.length > 0 ? <Scoreboard rows={tile.rows} /> : undefined,
        extra: (
          <span className="inline-flex min-w-0 items-center gap-1.5 font-bold tabular-nums">
            {tile.live && <LiveDot />}
            <span className="truncate">{status}</span>
          </span>
        ),
        action: tile.action || t("tiles.app.open"),
        left: tile.progress,
      };
    }
  }
}

/** Up to four faces, overlapping, each popping in as they arrive; then a count of the rest. */
function Faces({ instanceKey, userIds, flags }: { instanceKey: string; userIds: string[]; flags?: ReactNode }) {
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  const shown = userIds.slice(0, 4);
  const rest = userIds.length - shown.length;
  return (
    <>
      <span className="flex shrink-0 -space-x-1.5">
        <AnimatePresence initial={false}>
          {shown.map((id) => (
            <motion.span
              key={id}
              layout="position"
              initial={{ opacity: 0, scale: 0.4 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.4 }}
              transition={SPRING}
              className="rounded-full ring-2 ring-card"
            >
              <UserAvatar user={users?.[id]} className="size-5 text-[0.55rem]" />
            </motion.span>
          ))}
        </AnimatePresence>
      </span>
      {rest > 0 && (
        <span className="shrink-0 font-bold tabular-nums">
          +<Count value={rest} />
        </span>
      )}
      {flags}
    </>
  );
}

function VoiceHints({ video, screen }: { video: boolean; screen: boolean }) {
  return (
    <span className="flex items-center gap-1">
      {video && <VideoIcon className="size-3.5" />}
      {screen && <MonitorUpIcon className="size-3.5" />}
    </span>
  );
}

/** A thin bar along the bottom that shrinks as the moment comes (transform only). */
function Countdown({ left, tint }: { left: number; tint: string }) {
  return (
    <span className="absolute inset-x-0 bottom-0 h-0.5 bg-muted" aria-hidden>
      <motion.span
        className={cn("absolute inset-0 origin-left bg-gradient-to-r", tint)}
        initial={false}
        animate={{ scaleX: Math.min(1, Math.max(0, left)) }}
        transition={{ duration: 0.8, ease: [0.22, 1, 0.36, 1] }}
      />
    </span>
  );
}

/** One soft light sweeping across a tile as it arrives. */
function Sweep() {
  const reduce = useQuiet();
  if (reduce) return null;
  return (
    <motion.span
      aria-hidden
      className="pointer-events-none absolute inset-y-0 -left-1/2 w-1/2 bg-gradient-to-r from-transparent via-white/15 to-transparent"
      initial={{ x: "0%" }}
      animate={{ x: "400%" }}
      transition={{ duration: 1.4, ease: [0.22, 1, 0.36, 1], delay: 0.2 }}
    />
  );
}

function TileMenu({ tile, instanceKey, serverId }: { tile: Tile; instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const access = useAccess(instanceKey, serverId);
  const remove = removable(tile, (channelId) => hasIn(access, channelId, Permission.MANAGE_MESSAGES)) ? tile : null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={t("tiles.strip.options")}
          className="grid size-6 shrink-0 place-items-center rounded-md text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:bg-muted focus-visible:opacity-100 data-[state=open]:opacity-100 max-md:opacity-100"
        >
          <EllipsisIcon className="size-4" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-60">
        <DropdownMenuItem onSelect={() => hideTile(tile.id)}>
          <EyeOffIcon /> {t("tiles.strip.hide")}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => setServerQuiet(serverId, true)}>
          <BellOffIcon /> {t("tiles.strip.quiet")}
        </DropdownMenuItem>
        {remove && (
          <DropdownMenuItem
            className="text-destructive focus:text-destructive"
            onSelect={() =>
              void run(endLiveTile(instanceKey, serverId, remove.channelId, remove.sourceId, remove.tileId)).catch((e: { message?: string }) =>
                toast(e.message ?? t("tiles.strip.removeFailed")),
              )
            }
          >
            <Trash2Icon /> {t("tiles.strip.remove")}
          </DropdownMenuItem>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => setTilesOn(false)}>
          <PowerOffIcon /> {t("tiles.strip.off")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * In the server's menu: tiles here or not, for you; and for whoever can
 * manage the server, which kinds everyone here sees (a server setting).
 */
export function LiveTilesMenuItem({ instanceKey, serverId, manage }: { instanceKey: string; serverId: string; manage: boolean }) {
  const { t } = useI18n();
  const { on, quiet } = useLiveTilesLocal();
  const here = useTilesHere(instanceKey);
  const { customized, kinds } = useServerKinds(instanceKey, serverId);
  if (!here) return null;
  const save = (next: { customized: boolean; kinds: number[] }) =>
    run(updateServer(instanceKey, serverId, { liveTiles: next })).catch((e: { message?: string }) => toast(e.message ?? t("tiles.menu.failed")));
  const toggle = (kind: TileKind, show: boolean) => {
    const next = new Set(kinds);
    if (show) next.add(kind);
    else next.delete(kind);
    void save({ customized: true, kinds: kindNumbers(next) });
  };
  return (
    <>
      <DropdownMenuCheckboxItem
        checked={on && !quiet.has(serverId)}
        onCheckedChange={(show) => {
          if (show && !on) setTilesOn(true);
          setServerQuiet(serverId, !show);
        }}
        onSelect={(e) => e.preventDefault()}
      >
        {t("tiles.menu.show")}
      </DropdownMenuCheckboxItem>
      {manage && (
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <RadioTowerIcon /> {t("tiles.menu.server")}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-64">
            {TILE_KINDS.map((kind) => {
              const Icon = KIND[kind].icon;
              return (
                <DropdownMenuCheckboxItem key={kind} checked={kinds.has(kind)} onCheckedChange={(show) => toggle(kind, show)} onSelect={(e) => e.preventDefault()}>
                  <Icon /> {t(`tiles.kind.${kind}`)}
                </DropdownMenuCheckboxItem>
              );
            })}
            {customized && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem onSelect={() => void save({ customized: false, kinds: [] })}>
                  <RotateCcwIcon /> {t("tiles.menu.reset")}
                </DropdownMenuItem>
              </>
            )}
            <DropdownMenuLabel className="text-xs leading-4 font-normal text-muted-foreground">
              {customized ? t("tiles.menu.everyone") : t("tiles.menu.bigServer", { count: BIG_SERVER })}
            </DropdownMenuLabel>
          </DropdownMenuSubContent>
        </DropdownMenuSub>
      )}
    </>
  );
}

/**
 * While tiles show, the channel list slides down and up as they come and go
 * (a layout animation: transforms only). Without them, the list renders as it always did.
 */
export function LiveTilesGroup({ children }: { children: ReactNode }) {
  const { on } = useLiveTilesLocal();
  if (!on) return children;
  return <LayoutGroup id="live-tiles">{children}</LayoutGroup>;
}

/** An app's rows, such as teams and scores: a value that changes rolls to its new one. */
function Scoreboard({ rows }: { rows: { label: string; value: string }[] }) {
  return (
    <ul className="mx-2 mb-2 flex flex-col gap-0.5 rounded-lg bg-muted/50 px-2 py-1.5">
      {rows.map((row, n) => (
        <li key={n} className="flex items-center gap-2 text-sm">
          <span className="min-w-0 flex-1 truncate">{row.label}</span>
          <span className="relative inline-flex overflow-hidden font-extrabold tabular-nums">
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.span
                key={row.value}
                initial={{ y: "100%", opacity: 0, scale: 1.4 }}
                animate={{ y: 0, opacity: 1, scale: 1 }}
                exit={{ y: "-100%", opacity: 0 }}
                transition={SPRING}
                className="inline-block"
              >
                {row.value}
              </motion.span>
            </AnimatePresence>
          </span>
        </li>
      ))}
    </ul>
  );
}
