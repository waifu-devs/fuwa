import { useNavigate } from "@tanstack/react-router";
import { clock } from "@/voice/player";
import { ArrowRightIcon, HashIcon, HourglassIcon, PaperclipIcon, SearchXIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { SearchResult } from "@/gen/fuwa/v1/search_pb";
import type { Member, User } from "@/gen/fuwa/v1/types_pb";
import { useRoles } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { closeSearch, loadMoreResults, requestJump, useSearch } from "@/fuwa/search";
import { Mention, remarkMentions, ServerLookProvider, type ServerLook } from "@/components/chat/mentions";
import { UserAvatar } from "@/components/Icons";
import { Markdown, type MarkdownExtension } from "@/components/Markdown";
import { Count } from "@/components/motion";
import { T, useI18n } from "@/i18n/react";
import { remarkSearchHits } from "@/components/search/highlight";
import { SearchField } from "@/components/search/SearchBar";
import { displayName, formatStamp, memberName, toDate } from "@/lib/format";
import { reportTiming } from "@/lib/reports";
import { markRanges } from "@/lib/search-query";
import { cn } from "@/lib/utils";

/**
 * Search results for the server on screen, beside the chat (over it on a
 * phone): newest first, under the channel each is in, with the words found
 * highlighted. Clicking one opens its channel at that message. Results come
 * a page at a time as you scroll, and only the rows in view are drawn.
 */

const EMPTY: never[] = [];
/** Rows drawn beyond the edges of the view, in pixels. */
const OVERSCAN = 600;
/** A row's height before it's measured. */
const ESTIMATE = { channel: 32, result: 92 };
/** How many of the first rows fade in one after another. */
const STAGGER_ROWS = 10;

const SEARCH_TEXT: MarkdownExtension = {
  remarkPlugins: [remarkMentions, remarkSearchHits],
  components: { "fuwa-mention": Mention },
};

type Row =
  | { kind: "channel"; key: string; channelId: string; name: string }
  | { kind: "result"; key: string; result: SearchResult; index: number };

export function SearchPanel({ instanceKey, serverId, sheet = false }: { instanceKey: string; serverId: string; sheet?: boolean }) {
  const results = useSearch((s) => s.results);
  const total = useSearch((s) => s.total);
  const atLeast = useSearch((s) => s.totalAtLeast);
  const loading = useSearch((s) => s.loading);
  const loadingMore = useSearch((s) => s.more);
  const cursor = useSearch((s) => s.cursor);
  const error = useSearch((s) => s.error);
  const indexing = useSearch((s) => s.indexing);
  const indexedPercent = useSearch((s) => s.indexedPercent);
  const query = useSearch((s) => s.query);
  const run = useSearch((s) => s.run);
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[serverId] ?? EMPTY);
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? EMPTY);
  const emojis = useFuwa((s) => s.instances[instanceKey]?.emojis[serverId] ?? EMPTY);
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  const me = useFuwa((s) => s.instances[instanceKey]?.me ?? undefined);
  const ownerId = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.ownerId ?? "");
  const roles = useRoles(instanceKey, serverId);
  const navigate = useNavigate();
  const { t, number } = useI18n();

  const memberById = useMemo(() => new Map(members.map((m) => [m.user?.id ?? "", m])), [members]);
  const look = useMemo<ServerLook>(() => {
    const myRoles = memberById.get(me?.id ?? "")?.roleIds ?? [];
    return { instanceKey, ownerId, roles, members, emojis, me: me ? { id: me.id, username: me.username, roleIds: myRoles } : undefined };
  }, [instanceKey, ownerId, roles, members, emojis, me, memberById]);

  // Results under their channel: a heading wherever the channel changes, as Discord does.
  const rows = useMemo(() => {
    const out: Row[] = [];
    let last = "";
    results.forEach((result, index) => {
      const channelId = result.message?.channelId ?? "";
      if (channelId !== last) {
        out.push({ kind: "channel", key: `c-${index}-${channelId}`, channelId, name: channels.find((c) => c.id === channelId)?.name ?? t("chattools.search.channel") });
        last = channelId;
      }
      out.push({ kind: "result", key: result.message?.id ?? `r-${index}`, result, index });
    });
    return out;
  }, [results, channels, t]);

  const { scroller, layout, visible, measure, setView } = useWindowedRows(rows, run);

  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    setView({ top: el.scrollTop, height: el.clientHeight });
    if (cursor && !loading && el.scrollHeight - el.scrollTop - el.clientHeight < 800) void loadMoreResults();
  };

  const jump = useCallback(
    (result: SearchResult) => {
      const message = result.message;
      if (!message) return;
      const started = performance.now();
      requestJump(instanceKey, message.channelId, message.id);
      void navigate({ to: "/$instance/$server/$channel", params: { instance: instanceKey, server: serverId, channel: message.channelId } }).then(
        () => reportTiming("search.open_result", performance.now() - started),
      );
      if (sheet) closeSearch();
    },
    [instanceKey, serverId, navigate, sheet],
  );

  // Arrow keys walk the results; Enter opens one.
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const buttons = [...(scroller.current?.querySelectorAll<HTMLButtonElement>("[data-result]") ?? [])];
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = buttons[e.key === "ArrowDown" ? Math.min(buttons.length - 1, at + 1) : Math.max(0, at - 1)];
    if (next) {
      e.preventDefault();
      next.focus();
      next.scrollIntoView({ block: "nearest" });
    }
  };

  return (
    <ServerLookProvider value={look}>
      <div className="flex h-full min-h-0 flex-col" onKeyDown={onKeyDown}>
        <PanelHeader instanceKey={instanceKey} serverId={serverId} sheet={sheet} />
        {sheet && query && !loading && !error && (
          <p className="px-4 pt-2 text-sm font-bold">
            <T k={atLeast ? "chattools.search.resultsAtLeast" : "chattools.search.results"} values={{ count: <Count value={total} /> }} count={total} />
          </p>
        )}
        <AnimatePresence initial={false}>
          {indexing && (
            <motion.p
              key="indexing"
              initial={{ opacity: 0, y: -6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              className="mx-3 mt-2 flex items-center gap-2 rounded-xl bg-muted px-3 py-2 text-xs text-muted-foreground"
            >
              <HourglassIcon className="size-3.5 shrink-0" />
              <span>
                {t("chattools.search.indexing", { percent: number(indexedPercent / 100, { style: "percent" }) })}
              </span>
            </motion.p>
          )}
        </AnimatePresence>
        <div ref={scroller} onScroll={onScroll} className="scroll-thin relative min-h-0 flex-1 overflow-y-auto">
          <PanelState sheet={sheet} />
          <div className="relative" style={{ height: loading && !loadingMore ? 0 : layout.height }}>
            {visible.map(({ row, top }) => (
              <div key={row.key} ref={(el) => measure(row.key, el)} className="absolute inset-x-0 top-0" style={{ transform: `translateY(${top}px)` }}>
                {row.kind === "channel" ? (
                  <div className="flex items-center gap-1.5 px-4 pt-3 pb-1 text-xs font-extrabold text-muted-foreground">
                    <HashIcon className="size-3.5" />
                    {row.name}
                  </div>
                ) : (
                  <ResultRow
                    result={row.result}
                    stagger={row.index < STAGGER_ROWS ? row.index : -1}
                    run={run}
                    author={authorOf(row.result, memberById, users)}
                    member={memberById.get(row.result.message?.authorId ?? "")}
                    onOpen={jump}
                  />
                )}
              </div>
            ))}
          </div>
          {loadingMore && <Skeleton rows={2} />}
          <LookFurther />
        </div>
      </div>
    </ServerLookProvider>
  );
}

/** Draws only the rows in view, placed by their measured heights. */
function useWindowedRows(rows: Row[], run: number) {
  const scroller = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: 800 });
  const [heights, setHeights] = useState<Record<string, number>>({});
  const measure = useCallback((key: string, el: HTMLElement | null) => {
    if (!el) return;
    const h = el.offsetHeight;
    setHeights((cur) => (cur[key] === h || h === 0 ? cur : { ...cur, [key]: h }));
  }, []);
  const layout = useMemo(() => {
    let top = 0;
    const placed = rows.map((row) => {
      const at = top;
      top += heights[row.key] ?? ESTIMATE[row.kind];
      return { row, top: at };
    });
    return { placed, height: top };
  }, [rows, heights]);
  const visible = layout.placed.filter(
    ({ row, top }) => top + (heights[row.key] ?? ESTIMATE[row.kind]) >= view.top - OVERSCAN && top <= view.top + view.height + OVERSCAN,
  );

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const update = () => setView({ top: el.scrollTop, height: el.clientHeight });
    update();
    const resize = new ResizeObserver(update);
    resize.observe(el);
    return () => resize.disconnect();
  }, []);

  // A new search starts at the top.
  useEffect(() => {
    scroller.current?.scrollTo({ top: 0 });
    setView((v) => ({ ...v, top: 0 }));
  }, [run]);
  return { scroller, layout, visible, measure, setView };
}

function PanelHeader({ instanceKey, serverId, sheet }: { instanceKey: string; serverId: string; sheet: boolean }) {
  const total = useSearch((s) => s.total);
  const atLeast = useSearch((s) => s.totalAtLeast);
  const loading = useSearch((s) => s.loading);
  const loadingMore = useSearch((s) => s.more);
  const query = useSearch((s) => s.query);
  const { t } = useI18n();
  return (
    <header className="flex shrink-0 items-center gap-2 border-b px-3 py-2.5">
      {sheet ? (
        <SearchField instanceKey={instanceKey} serverId={serverId} inline autoFocus={!query} className="min-w-0 flex-1" />
      ) : (
        <h2 className="flex min-w-0 flex-1 items-baseline gap-1.5 font-extrabold">
          {loading && !loadingMore ? (
            <span className="text-muted-foreground">{t("chattools.search.searching")}</span>
          ) : (
            <span>
              <T k={atLeast ? "chattools.search.resultsAtLeast" : "chattools.search.results"} values={{ count: <Count value={total} /> }} count={total} />
            </span>
          )}
        </h2>
      )}
      <motion.button
        type="button"
        aria-label={t("chattools.search.close")}
        onClick={closeSearch}
        whileTap={{ scale: 0.85 }}
        className="grid size-8 shrink-0 place-items-center self-start rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
      >
        <XIcon className="size-4" />
      </motion.button>
    </header>
  );
}

/**
 * On a big server one search reads only so far back; this carries on from
 * there. Scrolling down does the same once there are results to scroll.
 */
function LookFurther() {
  const show = useSearch((s) => !!s.cursor && !s.loading && !s.error && s.results.length < 12);
  const { t } = useI18n();
  if (!show) return null;
  return (
    <motion.div initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} className="flex justify-center px-4 py-3">
      <button
        type="button"
        onClick={() => void loadMoreResults()}
        className="rounded-full bg-muted px-4 py-1.5 text-sm font-bold text-muted-foreground transition hover:bg-primary/10 hover:text-primary active:scale-95"
      >
        {t("chattools.search.further")}
      </button>
    </motion.div>
  );
}

/** What the list shows besides results: loading, an error, nothing found, or how to start. */
function PanelState({ sheet }: { sheet: boolean }) {
  const loading = useSearch((s) => s.loading && !s.more);
  const error = useSearch((s) => s.error);
  const query = useSearch((s) => s.query);
  const empty = useSearch((s) => s.results.length === 0);
  const further = useSearch((s) => !!s.cursor);
  const nothing = !loading && !error && empty && !!query;
  const { t } = useI18n();
  return (
    <>
      {loading && <Skeleton />}
      {error && <Empty icon={<SearchXIcon className="size-6" />} title={t("chattools.search.failed")} text={error} />}
      {nothing &&
        (further ? (
          <Empty icon={<SearchXIcon className="size-6" />} title={t("chattools.search.nothingNewest")} text={t("chattools.search.olderNotSearched")} />
        ) : (
          <Empty icon={<SearchXIcon className="size-6" />} title={t("chattools.search.nothingFound")} text={t("chattools.search.tryOther")} />
        ))}
      {!sheet && !query && !loading && !error && (
        <Empty icon={<HashIcon className="size-6" />} title={t("chattools.search.start")} text={t("chattools.search.startAbout")} />
      )}
    </>
  );
}

function authorOf(result: SearchResult, members: Map<string, Member>, users: Record<string, User> | undefined): User | undefined {
  const message = result.message;
  if (!message) return undefined;
  if (message.webhook) return { id: message.webhook.webhookId, username: message.webhook.name, displayName: message.webhook.name, avatarUrl: message.webhook.avatarUrl } as User;
  return members.get(message.authorId)?.user ?? users?.[message.authorId];
}

const ResultRow = memo(function ResultRow({
  result,
  stagger,
  run,
  author,
  member,
  onOpen,
}: {
  result: SearchResult;
  /** Its place among the first rows, which fade in one after another; -1 for the rest. */
  stagger: number;
  run: number;
  author: User | undefined;
  member: Member | undefined;
  onOpen: (result: SearchResult) => void;
}) {
  const message = result.message!;
  const date = toDate(message.createdAt);
  const text = useMemo(() => markRanges(message.content, result.highlights), [message.content, result.highlights]);
  const { t } = useI18n();
  return (
    <motion.div
      key={run}
      initial={stagger >= 0 ? { opacity: 0, y: 10 } : false}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", stiffness: 420, damping: 34, delay: Math.max(0, stagger) * 0.035 }}
      className="px-2 pb-1.5"
    >
      <button
        type="button"
        data-result
        onClick={() => onOpen(result)}
        className="group/result relative flex w-full gap-3 rounded-2xl border border-transparent bg-card/60 px-3 py-2.5 text-left transition-[background-color,border-color,transform] duration-150 hover:border-border hover:bg-card focus-visible:border-primary/50 focus-visible:outline-none active:scale-[0.99]"
      >
        <UserAvatar user={author} className="size-8 text-xs" />
        <span className="min-w-0 flex-1">
          <span className="flex items-baseline gap-2">
            <span className="truncate text-sm font-extrabold">{member ? memberName(member) : displayName(author)}</span>
            <time className="shrink-0 text-[0.7rem] text-muted-foreground" dateTime={date.toISOString()}>
              {formatStamp(date)}
            </time>
          </span>
          {message.content && <Markdown className="chat search-result line-clamp-6 text-sm" extension={SEARCH_TEXT}>{text}</Markdown>}
          {message.attachments.length > 0 && (
            <span className="mt-1 flex flex-wrap gap-1">
              {message.attachments.map((a) => (
                <span key={a.id} className="inline-flex max-w-full items-center gap-1 rounded-lg bg-muted px-2 py-0.5 text-xs text-muted-foreground">
                  <PaperclipIcon className="size-3 shrink-0" />
                  <span className="truncate">{a.voice ? t("chattools.search.voiceMessage", { duration: clock(a.voice.durationMs) }) : a.filename}</span>
                </span>
              ))}
            </span>
          )}
          {message.embeds.length > 0 && !message.content && (
            <span className="mt-1 block truncate text-xs text-muted-foreground">{message.embeds[0]!.title || message.embeds[0]!.description}</span>
          )}
        </span>
        <span className="pointer-events-none absolute top-2 right-2 flex items-center gap-1 rounded-full bg-primary px-2 py-0.5 text-[0.7rem] font-bold text-primary-foreground opacity-0 transition-[opacity,transform] duration-150 group-hover/result:translate-x-0 group-hover/result:opacity-100 group-focus-visible/result:opacity-100 translate-x-1">
          {t("chattools.search.jump")} <ArrowRightIcon className="size-3" />
        </span>
      </button>
    </motion.div>
  );
});

function Skeleton({ rows = 5 }: { rows?: number }) {
  return (
    <div className="space-y-2 px-3 py-3" aria-hidden>
      {Array.from({ length: rows }, (_, n) => (
        <motion.div
          key={n}
          initial={{ opacity: 0 }}
          animate={{ opacity: [0.35, 0.7, 0.35] }}
          transition={{ duration: 1.4, repeat: Infinity, delay: n * 0.12, ease: "easeInOut" }}
          className="flex gap-3 rounded-2xl bg-card/60 px-3 py-3"
        >
          <span className="size-8 shrink-0 rounded-full bg-muted" />
          <span className="flex-1 space-y-2">
            <span className="block h-3 w-1/3 rounded bg-muted" />
            <span className="block h-3 w-5/6 rounded bg-muted" />
          </span>
        </motion.div>
      ))}
    </div>
  );
}

function Empty({ icon, title, text }: { icon: React.ReactNode; title: string; text: string }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", stiffness: 380, damping: 30 }}
      className="flex flex-col items-center gap-2 px-6 py-12 text-center"
    >
      <motion.span
        initial={{ scale: 0.6, rotate: -12 }}
        animate={{ scale: 1, rotate: 0 }}
        transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.05 }}
        className={cn("grid size-12 place-items-center rounded-2xl bg-muted text-muted-foreground")}
      >
        {icon}
      </motion.span>
      <p className="font-extrabold">{title}</p>
      <p className="max-w-60 text-sm text-muted-foreground">{text}</p>
    </motion.div>
  );
}
