import { ArrowLeftIcon, ClockIcon, LoaderCircleIcon, SearchIcon, StarIcon, TrendingUpIcon, UploadIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { GifCategory, GifResult } from "@/gen/fuwa/v1/gif_pb";
import { MediaPurpose } from "@/gen/fuwa/v1/media_pb";
import type { MessageGif } from "@/gen/fuwa/v1/types_pb";
import { run, uploadPicture } from "@/fuwa/actions";
import { gifCategories, isSaved, prepareGif, saveGif, searchGifs, unsaveGif, useRecentGifs, useSavedGifs } from "@/fuwa/gifs";
import { GifImage } from "@/components/chat/GifImage";
import { SPRING } from "@/lib/motion";
import { useI18n } from "@/i18n/react";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/*
 * The GIF picker itself, in its own file: it is fetched once the app is idle
 * (or when the GIF button is first pressed), so it costs nothing at start.
 */

type Tab = "browse" | "saved" | "recent";

/** One tile in a grid: a search result, or a GIF ready to send. */
type Tile = {
  key: string;
  url: string;
  still?: string;
  width: number;
  height: number;
  title: string;
  /** A search result still to store, or a sealed GIF to send as it is. */
  pick: { result: string } | { gif: MessageGif };
  /** Its stored link, when it has one (to save or unsave it). */
  storedUrl?: string;
};

const fromResult = (r: GifResult): Tile => ({
  key: r.id,
  url: r.previewUrl,
  still: r.stillUrl,
  width: r.width,
  height: r.height,
  title: r.title,
  pick: { result: r.id },
});

const fromGif = (g: MessageGif): Tile => ({
  key: g.url,
  url: g.url,
  width: g.width,
  height: g.height,
  title: g.title,
  pick: { gif: g },
  storedUrl: g.url,
});


export function GifPanel({ instanceKey, credit, onSend }: { instanceKey: string; credit: string; onSend: (gif: MessageGif) => Promise<void> }) {
  const [tab, setTab] = useState<Tab>("browse");
  const [typed, setTyped] = useState("");
  const [query, setQuery] = useState("");
  /** Trending, browsed from its tile. */
  const [trending, setTrending] = useState(false);
  const [sending, setSending] = useState<string | null>(null);
  const saved = useSavedGifs(instanceKey);
  const recent = useRecentGifs(instanceKey);
  const { t } = useI18n();

  // Search as you type, a moment after you stop.
  useEffect(() => {
    const id = setTimeout(() => setQuery(typed.trim()), 300);
    return () => clearTimeout(id);
  }, [typed]);

  const pick = useCallback(
    async (tile: Tile) => {
      if (sending) return;
      setSending(tile.key);
      try {
        const gif = "gif" in tile.pick ? tile.pick.gif : await run(prepareGif(instanceKey, tile.pick.result));
        await onSend(gif);
      } catch (err) {
        toast((err as Error).message);
      } finally {
        setSending(null);
      }
    },
    [instanceKey, onSend, sending],
  );

  const toggleSave = useCallback(
    async (tile: Tile) => {
      try {
        let url = tile.storedUrl;
        if (!url && "result" in tile.pick) url = (await run(prepareGif(instanceKey, tile.pick.result))).url;
        if (!url) return;
        if (isSaved(saved.list, url)) await run(unsaveGif(instanceKey, url));
        else await run(saveGif(instanceKey, url));
      } catch (err) {
        toast((err as Error).message);
      }
    },
    [instanceKey, saved.list],
  );

  const searching = !!query || trending;
  const shown: Tab | "search" = searching ? "search" : tab;

  return (
    <>
      <div className="flex items-center gap-1.5 border-b p-2">
        <div className="relative flex-1">
          {/* Back takes the search glass's place, so nothing beside it moves. */}
          <AnimatePresence initial={false} mode="popLayout">
            {searching ? (
              <motion.button
                key="back"
                type="button"
                initial={{ opacity: 0, scale: 0.6, rotate: 45 }}
                animate={{ opacity: 1, scale: 1, rotate: 0 }}
                exit={{ opacity: 0, scale: 0.6 }}
                transition={SPRING}
                onClick={() => {
                  setTyped("");
                  setQuery("");
                  setTrending(false);
                }}
                aria-label={t("chattools.gifs.back")}
                className="absolute top-1.5 left-1 z-10 grid size-6 place-items-center rounded-lg text-muted-foreground hover:bg-muted hover:text-foreground"
              >
                <ArrowLeftIcon className="size-4" />
              </motion.button>
            ) : (
              <motion.span
                key="glass"
                initial={{ opacity: 0, scale: 0.6 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.6 }}
                transition={SPRING}
                className="pointer-events-none absolute top-2.5 left-2.5 text-muted-foreground"
              >
                <SearchIcon className="size-4" />
              </motion.span>
            )}
          </AnimatePresence>
          <input
            value={typed}
            onChange={(e) => {
              setTyped(e.target.value);
              setTrending(false);
            }}
            maxLength={100}
            placeholder={credit ? t("chattools.gifs.searchProvider", { provider: credit }) : t("chattools.gifs.search")}
            aria-label={t("chattools.gifs.search")}
            className="h-9 w-full rounded-xl bg-muted/60 pr-8 pl-8 text-sm outline-none focus:ring-2 focus:ring-primary/40"
          />
          {typed && (
            <button
              type="button"
              onClick={() => setTyped("")}
              aria-label={t("chattools.gifs.clear")}
              className="absolute top-1/2 right-1.5 grid size-6 -translate-y-1/2 place-items-center rounded-lg text-muted-foreground hover:bg-muted"
            >
              <XIcon className="size-3.5" />
            </button>
          )}
        </div>
      </div>
      {!searching && <Tabs tab={tab} onTab={setTab} savedCount={saved.list.length} />}
      <div className="relative min-h-0 flex-1">
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.div
            key={shown === "search" ? `search:${query}:${trending}` : shown}
            initial={{ opacity: 0, x: 12 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -12 }}
            transition={SPRING}
            className="absolute inset-0"
          >
            {shown === "search" ? (
              <Results instanceKey={instanceKey} query={trending ? "" : query} saved={saved.list} sending={sending} onPick={pick} onSave={toggleSave} />
            ) : shown === "browse" ? (
              <Browse
                instanceKey={instanceKey}
                onTrending={() => setTrending(true)}
                onCategory={(c) => {
                  setTyped(c.query);
                  setQuery(c.query);
                }}
              />
            ) : shown === "saved" ? (
              <Saved instanceKey={instanceKey} sending={sending} onPick={pick} onSave={toggleSave} />
            ) : (
              <Grid
                tiles={recent.map(fromGif)}
                saved={saved.list}
                sending={sending}
                onPick={pick}
                onSave={toggleSave}
                empty={<Empty icon={<ClockIcon className="size-6" />} title={t("chattools.gifs.nothingSent")} text={t("chattools.gifs.nothingSentAbout")} />}
              />
            )}
          </motion.div>
        </AnimatePresence>
      </div>
      {credit && (
        <p className="flex h-7 shrink-0 items-center justify-end border-t px-3 text-[0.65rem] font-bold tracking-wide text-muted-foreground">
          {t("chattools.gifs.credit", { provider: credit })}
        </p>
      )}
    </>
  );
}

function Tabs({ tab, onTab, savedCount }: { tab: Tab; onTab: (t: Tab) => void; savedCount: number }) {
  const { t } = useI18n();
  const tabs: { id: Tab; label: string; icon: ReactNode }[] = [
    { id: "browse", label: t("chattools.gifs.browse"), icon: <TrendingUpIcon className="size-3.5" /> },
    { id: "saved", label: savedCount ? t("chattools.gifs.savedCount", { count: savedCount }) : t("chattools.gifs.saved"), icon: <StarIcon className="size-3.5" /> },
    { id: "recent", label: t("chattools.gifs.recent"), icon: <ClockIcon className="size-3.5" /> },
  ];
  return (
    <div role="tablist" className="flex gap-1 px-2 pt-2">
      {tabs.map((item) => (
        <button
          key={item.id}
          type="button"
          role="tab"
          aria-selected={tab === item.id}
          onClick={() => onTab(item.id)}
          className={cn(
            "relative flex h-8 flex-1 items-center justify-center gap-1.5 rounded-lg text-xs font-bold transition-colors",
            tab === item.id ? "text-primary" : "text-muted-foreground hover:text-foreground",
          )}
        >
          {tab === item.id && <motion.span layoutId="gif-tab" transition={SPRING} className="absolute inset-0 rounded-lg bg-primary/10" />}
          <span className="relative flex items-center gap-1.5">
            {item.icon}
            {item.label}
          </span>
        </button>
      ))}
    </div>
  );
}

function Empty({ icon, title, text }: { icon: ReactNode; title: string; text: string }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={SPRING}
      className="flex h-full flex-col items-center justify-center gap-1.5 px-6 text-center"
    >
      <span className="mb-1 grid size-12 place-items-center rounded-2xl bg-muted text-muted-foreground">{icon}</span>
      <p className="text-sm font-bold">{title}</p>
      <p className="text-xs text-muted-foreground">{text}</p>
    </motion.div>
  );
}

/** Moods to browse, and trending first. */
function Browse({ instanceKey, onTrending, onCategory }: { instanceKey: string; onTrending: () => void; onCategory: (c: GifCategory) => void }) {
  const [categories, setCategories] = useState<GifCategory[] | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const still = usePrefs(reduceMotion);
  const { t } = useI18n();
  useEffect(() => {
    let live = true;
    gifCategories(instanceKey).then(
      (c) => live && setCategories(c),
      (err: Error) => live && setFailed(err.message),
    );
    return () => {
      live = false;
    };
  }, [instanceKey]);
  if (failed) return <Empty icon={<SearchIcon className="size-6" />} title={t("chattools.gifs.browseFailed")} text={failed} />;
  const tiles = categories ?? [];
  const trendingPicture = tiles[0]?.preview;
  return (
    <div className="scroll-thin grid h-full auto-rows-[5.5rem] grid-cols-2 gap-1.5 overflow-y-auto p-2">
      <CategoryTile n={0} label={t("chattools.gifs.trending")} icon={<TrendingUpIcon className="size-4" />} picture={trendingPicture} still={still} onClick={onTrending} />
      {categories
        ? tiles.map((c, n) => <CategoryTile key={c.query} n={n + 1} label={c.name} picture={c.preview} still={still} onClick={() => onCategory(c)} />)
        : Array.from({ length: 9 }, (_, n) => <span key={n} className="animate-pulse rounded-xl bg-muted" style={{ animationDelay: `${n * 60}ms` }} />)}
    </div>
  );
}

function CategoryTile({
  n,
  label,
  icon,
  picture,
  still,
  onClick,
}: {
  n: number;
  label: string;
  icon?: ReactNode;
  picture: GifResult | undefined;
  still: boolean;
  onClick: () => void;
}) {
  const [hover, setHover] = useState(false);
  return (
    <motion.button
      type="button"
      initial={{ opacity: 0, scale: 0.9 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{ ...SPRING, delay: Math.min(n, 12) * 0.025 }}
      whileHover={{ scale: 1.03 }}
      whileTap={{ scale: 0.96 }}
      onClick={onClick}
      onPointerEnter={() => setHover(true)}
      onPointerLeave={() => setHover(false)}
      className="relative overflow-hidden rounded-xl bg-muted text-left"
    >
      {picture && (
        <GifImage src={picture.previewUrl} still={picture.stillUrl} playing={!still || hover} alt="" className="absolute inset-0 size-full object-cover" />
      )}
      <span className="absolute inset-0 bg-gradient-to-t from-black/70 via-black/25 to-black/10" />
      <span className="absolute bottom-2 left-2.5 flex items-center gap-1.5 text-sm font-extrabold text-white [text-shadow:0_1px_3px_rgb(0_0_0/0.5)]">
        {icon}
        {label}
      </span>
    </motion.button>
  );
}

/** Search results (or trending), a page at a time as you scroll. */
function Results({
  instanceKey,
  query,
  saved,
  sending,
  onPick,
  onSave,
}: {
  instanceKey: string;
  query: string;
  saved: { gif?: MessageGif }[];
  sending: string | null;
  onPick: (t: Tile) => void;
  onSave: (t: Tile) => void;
}) {
  const [tiles, setTiles] = useState<Tile[]>([]);
  const [next, setNext] = useState<string | null>("");
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const asked = useRef(new Set<string>());
  const { t } = useI18n();

  const more = useCallback(() => {
    if (busy || next === null || asked.current.has(next)) return;
    const cursor = next;
    asked.current.add(cursor);
    setBusy(true);
    run(searchGifs(instanceKey, query, cursor)).then(
      (page) => {
        setTiles((t) => {
          const seen = new Set(t.map((x) => x.key));
          return [...t, ...page.results.map(fromResult).filter((x) => !seen.has(x.key))];
        });
        setNext(page.nextCursor || null);
        setBusy(false);
      },
      (err: Error) => {
        setFailed(err.message);
        setBusy(false);
      },
    );
  }, [busy, instanceKey, next, query]);

  useEffect(() => more(), []); // eslint-disable-line react-hooks/exhaustive-deps

  if (failed && !tiles.length) return <Empty icon={<SearchIcon className="size-6" />} title={t("chattools.gifs.searchFailed")} text={failed} />;
  if (!busy && !tiles.length && next === null)
    return <Empty icon={<SearchIcon className="size-6" />} title={t("chattools.gifs.noResults", { query })} text={t("chattools.gifs.tryOther")} />;
  return (
    <Grid
      tiles={tiles}
      saved={saved}
      sending={sending}
      onPick={onPick}
      onSave={onSave}
      onEnd={more}
      loading={busy}
      empty={<Skeletons />}
    />
  );
}

function Skeletons() {
  return (
    <div className="grid grid-cols-2 gap-1.5 p-2">
      {[96, 132, 120, 88, 140, 104].map((h, n) => (
        <span key={n} className="animate-pulse rounded-xl bg-muted" style={{ height: h, animationDelay: `${n * 70}ms` }} />
      ))}
    </div>
  );
}

/** Your saved GIFs, and a tile to upload one of your own. */
function Saved({
  instanceKey,
  sending,
  onPick,
  onSave,
}: {
  instanceKey: string;
  sending: string | null;
  onPick: (t: Tile) => void;
  onSave: (t: Tile) => void;
}) {
  const saved = useSavedGifs(instanceKey);
  const [progress, setProgress] = useState<number | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const { t, number } = useI18n();

  async function upload(file: File) {
    if (file.type !== "image/gif") return toast(t("chattools.gifs.pickGif"));
    setProgress(0);
    try {
      const url = await run(uploadPicture(instanceKey, MediaPurpose.GIF, file, setProgress));
      await run(saveGif(instanceKey, url));
      toast(t("chattools.gifs.uploaded"));
    } catch (err) {
      toast((err as Error).message);
    } finally {
      setProgress(null);
    }
  }

  const uploadTile = (
    <button
      type="button"
      onClick={() => input.current?.click()}
      disabled={progress !== null}
      className="flex h-full w-full flex-col items-center justify-center gap-1 rounded-xl border-2 border-dashed text-xs font-bold text-muted-foreground transition-colors hover:border-primary hover:text-primary"
    >
      {progress !== null ? <LoaderCircleIcon className="size-5 animate-spin" /> : <UploadIcon className="size-5" />}
      {progress !== null ? number(Math.round(progress * 100) / 100, { style: "percent" }) : t("chattools.gifs.upload")}
      <input
        ref={input}
        type="file"
        accept="image/gif"
        hidden
        onChange={(e) => {
          const file = e.target.files?.[0];
          e.target.value = "";
          if (file) void upload(file);
        }}
      />
    </button>
  );
  if (!saved.loaded) return <Skeletons />;
  return (
    <Grid
      tiles={saved.list.flatMap((s) => (s.gif ? [fromGif(s.gif)] : []))}
      saved={saved.list}
      sending={sending}
      onPick={onPick}
      onSave={onSave}
      lead={uploadTile}
      empty={null}
    />
  );
}

const GAP = 6;
const PAD = 8;
const OVERSCAN = 480;

/**
 * A masonry of GIFs in two columns, each tile sized from its known width and
 * height so nothing moves as they load. Only tiles near the view are drawn;
 * the rest are space. `onEnd` hears when the bottom comes near.
 */
function Grid({
  tiles,
  saved,
  sending,
  onPick,
  onSave,
  onEnd,
  loading,
  empty,
  lead,
}: {
  tiles: Tile[];
  saved: { gif?: MessageGif }[];
  sending: string | null;
  onPick: (t: Tile) => void;
  onSave: (t: Tile) => void;
  onEnd?: () => void;
  loading?: boolean;
  empty: ReactNode;
  /** A tile before the GIFs (upload). */
  lead?: ReactNode;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [view, setView] = useState({ top: 0, height: 0 });
  const still = usePrefs(reduceMotion);

  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const measure = () => {
      setWidth(el.clientWidth);
      setView({ top: el.scrollTop, height: el.clientHeight });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const layout = useMemo(() => {
    const column = Math.max(60, (width - PAD * 2 - GAP) / 2);
    const heights = [PAD, PAD];
    const placed: { tile: Tile | null; x: number; y: number; w: number; h: number }[] = [];
    const place = (tile: Tile | null, ratio: number) => {
      const c = heights[0]! <= heights[1]! ? 0 : 1;
      const h = Math.round(column * Math.min(2.2, Math.max(0.45, ratio)));
      placed.push({ tile, x: PAD + c * (column + GAP), y: heights[c]!, w: column, h });
      heights[c] = heights[c]! + h + GAP;
    };
    if (lead) place(null, 0.6);
    for (const t of tiles) place(t, t.width > 0 && t.height > 0 ? t.height / t.width : 1);
    return { placed, height: Math.max(heights[0]!, heights[1]!) + PAD };
  }, [lead, tiles, width]);

  const near = layout.placed.filter((p) => p.y + p.h > view.top - OVERSCAN && p.y < view.top + view.height + OVERSCAN);

  useEffect(() => {
    if (onEnd && width && view.top + view.height > layout.height - 600) onEnd();
  }, [layout.height, onEnd, view, width]);

  const savedUrls = useMemo(() => new Set(saved.map((s) => s.gif?.url ?? "")), [saved]);

  return (
    <div
      ref={box}
      onScroll={(e) => {
        const el = e.currentTarget;
        setView((v) => (Math.abs(v.top - el.scrollTop) < 40 && v.height === el.clientHeight ? v : { top: el.scrollTop, height: el.clientHeight }));
      }}
      className="scroll-thin relative h-full overflow-y-auto overscroll-contain"
    >
      {!tiles.length && !lead ? (
        empty
      ) : (
        <div style={{ height: layout.height }} className="relative">
          {near.map((p, n) =>
            p.tile ? (
              <GifTile
                key={p.tile.key}
                tile={p.tile}
                n={n}
                style={{ width: p.w, height: p.h, transform: `translate(${p.x}px, ${p.y}px)` }}
                still={still}
                saved={!!p.tile.storedUrl && savedUrls.has(p.tile.storedUrl)}
                sending={sending === p.tile.key}
                dimmed={!!sending && sending !== p.tile.key}
                onPick={onPick}
                onSave={onSave}
              />
            ) : (
              <div key="lead" className="absolute top-0 left-0" style={{ width: p.w, height: p.h, transform: `translate(${p.x}px, ${p.y}px)` }}>
                {lead}
              </div>
            ),
          )}
          {loading && (
            <LoaderCircleIcon className="absolute bottom-2 left-1/2 size-5 -translate-x-1/2 animate-spin text-muted-foreground" />
          )}
        </div>
      )}
    </div>
  );
}

function GifTile({
  tile,
  n,
  style,
  still,
  saved,
  sending,
  dimmed,
  onPick,
  onSave,
}: {
  tile: Tile;
  n: number;
  style: React.CSSProperties;
  still: boolean;
  saved: boolean;
  sending: boolean;
  dimmed: boolean;
  onPick: (t: Tile) => void;
  onSave: (t: Tile) => void;
}) {
  const [hover, setHover] = useState(false);
  const { t } = useI18n();
  return (
    <div className="group/tile absolute top-0 left-0" style={style} onPointerEnter={() => setHover(true)} onPointerLeave={() => setHover(false)}>
      <motion.button
        type="button"
        initial={{ opacity: 0, scale: 0.94 }}
        animate={{ opacity: dimmed ? 0.45 : 1, scale: sending ? 0.94 : 1 }}
        transition={{ ...SPRING, delay: Math.min(n, 10) * 0.02 }}
        whileHover={{ scale: 1.03 }}
        whileTap={{ scale: 0.95 }}
        onClick={() => onPick(tile)}
        onFocus={() => setHover(true)}
        onBlur={() => setHover(false)}
        aria-label={tile.title ? t("chattools.gifs.send", { title: tile.title }) : t("chattools.gifs.sendThis")}
        title={tile.title || undefined}
        className="relative size-full overflow-hidden rounded-xl bg-muted ring-primary/60 hover:ring-2 focus-visible:ring-2 focus-visible:outline-none"
      >
        <GifImage src={tile.url} still={tile.still} playing={!still || hover} alt={tile.title} className="size-full object-cover" />
        {sending && (
          <span className="absolute inset-0 grid place-items-center bg-black/35">
            <LoaderCircleIcon className="size-6 animate-spin text-white" />
          </span>
        )}
      </motion.button>
      <button
        type="button"
        onClick={() => onSave(tile)}
        aria-label={saved ? t("chattools.gifs.unsave") : t("chattools.gifs.save")}
        aria-pressed={saved}
        className={cn(
          "absolute top-1.5 right-1.5 grid size-7 place-items-center rounded-lg bg-black/55 text-white opacity-0 transition-[opacity,transform] duration-150 group-hover/tile:opacity-100 focus-visible:opacity-100 active:scale-90",
          saved && "opacity-100",
        )}
      >
        <StarIcon className={cn("size-3.5", saved && "fill-amber-400 text-amber-400")} />
      </button>
    </div>
  );
}
