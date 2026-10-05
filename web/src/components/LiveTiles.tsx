import { useNavigate } from "@tanstack/react-router";
import { BellOffIcon, CalendarClockIcon, ChartColumnIcon, ChevronRightIcon, EllipsisIcon, EyeOffIcon, MessagesSquareIcon, MonitorUpIcon, PowerOffIcon, RadioTowerIcon, VideoIcon, Volume2Icon } from "lucide-react";
import { AnimatePresence, LayoutGroup, motion, useReducedMotion } from "motion/react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { joinCall } from "@/calls/engine";
import { useCalls } from "@/calls/state";
import { Count, SPRING } from "@/components/motion";
import { useLayout } from "@/components/Shell";
import { UserAvatar } from "@/components/Icons";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { loadFollowed, run } from "@/fuwa/actions";
import { usePresenceSettings } from "@/fuwa/presence";
import { notificationKey, useFuwa } from "@/fuwa/store";
import { PresenceStatus } from "@/gen/fuwa/v1/presence_pb";
import { useI18n } from "@/i18n/react";
import { formatTime } from "@/lib/format";
import { collectTiles, EVENT_AHEAD_MS, pickTiles, POLL_SOON_MS, sampleTiles, type Tile, type TileKind } from "@/lib/live-tiles";
import { hideTile, setServerQuiet, setTilesMode, useLiveTilesLocal } from "@/lib/live-tiles-store";
import { isMuted, useNow } from "@/lib/notifications";
import { usePrefs } from "@/lib/prefs";
import { requestThread } from "@/lib/threads";
import { cn } from "@/lib/utils";

/**
 * Live tiles at the top of a server's channel list (see lib/live-tiles.ts):
 * a draft behind a switch in this browser, so nothing shows unless someone
 * turned it on. Quiet by design: at most three, none while you're on Do not
 * disturb or have the server muted, and each one hides with a click.
 */

const KIND: Record<TileKind, { icon: typeof Volume2Icon; tint: string }> = {
  event: { icon: CalendarClockIcon, tint: "from-fuchsia-500 to-pink-500" },
  voice: { icon: Volume2Icon, tint: "from-emerald-500 to-teal-500" },
  poll: { icon: ChartColumnIcon, tint: "from-amber-500 to-orange-500" },
  thread: { icon: MessagesSquareIcon, tint: "from-sky-500 to-indigo-500" },
  shared: { icon: RadioTowerIcon, tint: "from-violet-500 to-purple-500" },
};

const NO_VOICE: never[] = [];

export function LiveTiles({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { mode, hidden, quiet } = useLiveTilesLocal();
  if (mode === "off" || quiet.has(serverId)) return null;
  return <Strip instanceKey={instanceKey} serverId={serverId} demo={mode === "demo"} hidden={hidden} />;
}

function Strip({ instanceKey, serverId, demo, hidden }: { instanceKey: string; serverId: string; demo: boolean; hidden: ReadonlySet<string> }) {
  const { t } = useI18n();
  // A minute is fine for countdowns in minutes; rooms and threads move with the store.
  const now = useNow(15_000);
  const [anchor] = useState(Date.now);
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[serverId]);
  const voice = useFuwa((s) => s.instances[instanceKey]?.voice[serverId] ?? NO_VOICE);
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
      },
      now,
    );
    const all = demo ? [...real, ...sampleTiles(channels, anchor, t("tiles.event.sampleTitle"))] : real;
    return pickTiles(all, hidden, now);
  }, [channels, voice, messages, threadParents, followed, threadUnread, unread, notifications, inChannel, serverId, now, demo, anchor, hidden, t]);

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
      {tile.kind === "event" && <Sweep />}
      <div className="flex items-start gap-2 p-2">
        <button type="button" onClick={open} className="flex min-w-0 flex-1 items-start gap-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/60 rounded-lg">
          <span className={cn("grid size-8 shrink-0 place-items-center rounded-lg bg-gradient-to-br text-white shadow-sm", tint)}>
            <Icon className="size-4 transition-transform duration-300 group-hover:scale-110 group-hover:-rotate-6" />
          </span>
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-1.5">
              <span className="truncate text-sm leading-5 font-bold">{body.title}</span>
              {tile.sample && (
                <span title={t("tiles.strip.sampleTitle")} className="shrink-0 rounded-full border border-dashed px-1.5 text-[0.6rem] leading-4 font-bold text-muted-foreground uppercase">
                  {t("tiles.strip.sample")}
                </span>
              )}
            </span>
            <span className="block truncate text-xs leading-4 text-muted-foreground">{body.line}</span>
          </span>
        </button>
        <TileMenu tile={tile} serverId={serverId} />
      </div>
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

type Body = { title: string; line: string; extra: ReactNode; action: string; left: number | null };

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
    case "event": {
      const until = tile.startsAt - now;
      const minutes = Math.max(1, Math.ceil(until / 60_000));
      return {
        title: tile.title,
        line: until <= 0 ? t("tiles.event.now") : t("tiles.event.startsIn", { count: minutes }),
        extra: <span className="truncate">{t("tiles.event.going", { count: tile.going })}</span>,
        action: t("tiles.event.open"),
        left: until <= 0 ? null : until / EVENT_AHEAD_MS,
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

function TileMenu({ tile, serverId }: { tile: Tile; serverId: string }) {
  const { t } = useI18n();
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
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => setTilesMode("off")}>
          <PowerOffIcon /> {t("tiles.strip.off")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** In the server's menu while tiles are on in this browser: show them here or not. */
export function LiveTilesMenuItem({ serverId }: { serverId: string }) {
  const { t } = useI18n();
  const { mode, quiet } = useLiveTilesLocal();
  if (mode === "off") return null;
  return (
    <DropdownMenuCheckboxItem checked={!quiet.has(serverId)} onCheckedChange={(on) => setServerQuiet(serverId, !on)} onSelect={(e) => e.preventDefault()}>
      {t("tiles.menu.show")}
    </DropdownMenuCheckboxItem>
  );
}

/**
 * While tiles are on, the channel list slides down and up as they come and go
 * (a layout animation: transforms only). Off, the list renders as it always did.
 */
export function LiveTilesGroup({ children }: { children: ReactNode }) {
  const { mode } = useLiveTilesLocal();
  if (mode === "off") return children;
  return (
    <LayoutGroup id="live-tiles">{children}</LayoutGroup>
  );
}
