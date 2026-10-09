import { useNavigate } from "@tanstack/react-router";
import {
  ArrowDownIcon,
  ArrowRightIcon,
  ArrowUpIcon,
  CheckIcon,
  ChevronDownIcon,
  DownloadIcon,
  EyeOffIcon,
  GlobeIcon,
  HardDriveIcon,
  ImageIcon,
  LoaderCircleIcon,
  MapPinIcon,
  PlaneIcon,
  SearchIcon,
  ServerIcon as ServersIcon,
  StarIcon,
  Trash2Icon,
  TriangleAlertIcon,
  UnlinkIcon,
  UsersIcon,
} from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import type { InstanceServer } from "@/gen/fuwa/v1/admin_pb";
import { SharedConnectionState, type SharedConnection } from "@/gen/fuwa/v1/channel_pb";
import type { ServerLimits } from "@/gen/fuwa/v1/types_pb";
import type { Region } from "@/gen/fuwa/v1/types_pb";
import {
  deleteServer,
  endServerShare,
  exportServer,
  listInstanceServers,
  listServerShares,
  moveServer,
  nodeUsage,
  run,
  serverUsage,
  setFeaturedServers,
  setServerLimits,
} from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction, useInstance } from "@/fuwa/hooks";
import { ServerIcon } from "@/components/Icons";
import { SharedGlyph } from "@/components/chat/Shared";
import { ConfirmDialog, StateChip } from "@/components/settings/server/SharedChannels";
import { Count, CountUp } from "@/components/motion";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { Segmented } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { displayName, formatBytes, formatDay, toDate } from "@/lib/format";
import { T, useI18n } from "@/i18n/react";
import { toast } from "@/lib/ui";
import { hasRegions, regionMark, regionName, sameRegion } from "@/lib/regions";
import { instanceHas } from "@/lib/compat";
import { cn } from "@/lib/utils";
import { Cap } from "../controls";

type Sort = "storage" | "members" | "newest";
type Caps = Omit<ServerLimits, "$typeName">;
const CAP_FIELDS = ["members", "channels", "storageBytes", "attachmentBytes", "emojis", "recordingBytes"] as const;
const caps = (l: ServerLimits | undefined): Caps => ({
  members: l?.members,
  channels: l?.channels,
  storageBytes: l?.storageBytes,
  attachmentBytes: l?.attachmentBytes,
  emojis: l?.emojis,
  recordingBytes: l?.recordingBytes,
});

const storageOf = (s: InstanceServer) => Number(s.usage?.storageBytes ?? 0n);
const membersOf = (s: InstanceServer) => Number(s.usage?.members ?? s.server?.memberCount ?? 0n);

/**
 * Every community server on the instance, with its owner and what it holds.
 * Admins can change a server's caps, save its whole file, or delete it,
 * whether or not they're in it.
 */
export function Servers({ instanceKey, onLeave }: { instanceKey: string; onLeave: () => void }) {
  const lang = useI18n();
  const { t } = lang;
  const [servers, setServers] = useState<InstanceServer[] | null>(null);
  const [featured, setFeatured] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<Sort>("storage");
  const [open, setOpen] = useState<string | null>(null);
  const [pictures, setPictures] = useState({ count: 0, bytes: 0 });
  const node = useInstance(instanceKey)?.node;
  const regions = node?.regions ?? [];
  const canFeature = instanceHas(node?.versions, "featured-servers");

  useEffect(() => {
    run(listInstanceServers(instanceKey)).then(
      (r) => {
        setServers(r.servers);
        setFeatured(r.featured);
      },
      (e: FuwaError) => setError(e.message),
    );
    run(nodeUsage(instanceKey)).then(
      (u) => setPictures({ count: Number(u.pictures), bytes: Number(u.pictureBytes) }),
      () => {},
    );
  }, [instanceKey]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    const kept = (servers ?? []).filter(
      (s) => !q || `${s.server?.name ?? ""} ${s.owner?.username ?? ""} ${s.owner?.displayName ?? ""}`.toLowerCase().includes(q),
    );
    const by: Record<Sort, (a: InstanceServer, b: InstanceServer) => number> = {
      storage: (a, b) => storageOf(b) - storageOf(a),
      members: (a, b) => membersOf(b) - membersOf(a),
      newest: (a, b) => toDate(b.server?.createdAt).getTime() - toDate(a.server?.createdAt).getTime(),
    };
    return kept.sort(by[sort]);
  }, [servers, query, sort]);

  const biggest = Math.max(1, ...(servers ?? []).map(storageOf));
  const totals = [
    { label: t("instancesettings.nav.servers"), value: servers?.length ?? 0, icon: ServersIcon },
    { label: t("instancesettings.servers.memberships"), value: (servers ?? []).reduce((n, s) => n + membersOf(s), 0), icon: UsersIcon },
    { label: t("serversettings.usage.storage"), value: (servers ?? []).reduce((n, s) => n + storageOf(s), 0), icon: HardDriveIcon, bytes: true },
    { label: t("instancesettings.servers.pictures"), value: pictures.bytes, icon: ImageIcon, bytes: true, sub: t("instancesettings.servers.picturesSub", { count: pictures.count }) },
  ];

  if (error) return <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>;
  if (!servers) {
    return (
      <div className="flex flex-col gap-3">
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
          {[0, 1, 2, 3].map((n) => (
            <div key={n} className="shimmer h-20 rounded-2xl" />
          ))}
        </div>
        {[0, 1, 2].map((n) => (
          <div key={n} className="shimmer h-16 rounded-2xl" />
        ))}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        {totals.map((total, n) => (
          <motion.div
            key={total.label}
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ ...SPRING, delay: n * 0.05 }}
            title={total.sub}
            className="rounded-2xl border bg-background/50 p-3"
          >
            <p className="flex items-center gap-1.5 text-[0.65rem] font-bold tracking-wide text-muted-foreground uppercase">
              <total.icon className="size-3.5" /> {total.label}
            </p>
            <p className="mt-1 truncate text-xl font-extrabold tabular-nums">
              <CountUp value={total.value} delay={0.1 + n * 0.05} format={total.bytes ? (v) => formatBytes(lang, Math.round(v)) : undefined} />
            </p>
          </motion.div>
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-0 flex-1 basis-56">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("instancesettings.servers.search")} aria-label={t("instancesettings.servers.searchLabel")} className="h-10 rounded-xl pl-9" />
        </div>
        <Segmented
          label={t("instancesettings.servers.sortBy")}
          value={sort}
          onChange={setSort}
          options={[
            { value: "storage", label: t("instancesettings.servers.biggest") },
            { value: "members", label: t("serversettings.nav.members") },
            { value: "newest", label: t("instancesettings.servers.newest") },
          ]}
        />
      </div>

      <p className="flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
        <ServersIcon className="size-3.5" /> <T k="instancesettings.servers.count" values={{ count: <Count value={shown.length} /> }} count={shown.length} />
      </p>
      <ul className="flex flex-col gap-1.5">
        <AnimatePresence initial={false} mode="popLayout">
          {shown.map((s, n) => (
            <ServerRow
              key={s.server?.id}
              instanceKey={instanceKey}
              entry={s}
              index={n}
              biggest={biggest}
              open={open === s.server?.id}
              onToggle={() => setOpen((o) => (o === s.server?.id ? null : (s.server?.id ?? null)))}
              regions={regions}
              featured={canFeature ? featured : null}
              onFeatured={setFeatured}
              onMoved={(server) => setServers((list) => (list ?? []).map((x) => (x.server?.id === server.id ? { ...x, server } : x)))}
              onLimits={(limits) => setServers((list) => (list ?? []).map((x) => (x.server?.id === s.server?.id ? { ...x, limits } : x)))}
              onDeleted={() => setServers((list) => (list ?? []).filter((x) => x.server?.id !== s.server?.id))}
              onLeave={onLeave}
            />
          ))}
        </AnimatePresence>
      </ul>
      <AnimatePresence>
        {shown.length === 0 && (
          <motion.p initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} className="py-8 text-center text-sm text-muted-foreground">
            {t(query ? "instancesettings.servers.noMatch" : "instancesettings.servers.none")}
          </motion.p>
        )}
      </AnimatePresence>
    </div>
  );
}

function ServerRow({
  instanceKey,
  entry,
  index,
  biggest,
  open,
  regions,
  featured,
  onFeatured,
  onToggle,
  onMoved,
  onLimits,
  onDeleted,
  onLeave,
}: {
  instanceKey: string;
  entry: InstanceServer;
  index: number;
  regions: Region[];
  /** The featured servers' ids in order, or null where the instance can't feature any. */
  featured: string[] | null;
  onFeatured: (ids: string[]) => void;
  onMoved: (server: NonNullable<InstanceServer["server"]>) => void;
  biggest: number;
  open: boolean;
  onToggle: () => void;
  onLimits: (limits: ServerLimits) => void;
  onDeleted: () => void;
  onLeave: () => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const s = entry.server!;
  const storage = storageOf(entry);
  const cap = entry.limits?.storageBytes === undefined ? null : Number(entry.limits.storageBytes);
  const share = cap ? Math.min(1, storage / cap) : storage / biggest;
  const full = cap !== null && share >= 0.9;
  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: Math.min(index, 14) * 0.02 } }}
      exit={{ opacity: 0, scale: 0.96, x: -24, transition: { duration: 0.25 } }}
      transition={SPRING}
      className={cn("overflow-hidden rounded-2xl border bg-background/40 transition-colors", open ? "border-primary/40" : "hover:border-primary/30 hover:bg-muted/40")}
    >
      <button type="button" onClick={onToggle} aria-expanded={open} className="group relative flex w-full items-center gap-3 p-2.5 pr-3 text-left">
        <ServerIcon server={s} className="size-10 rounded-xl text-sm transition-transform duration-300 group-hover:scale-105" />
        <span className="min-w-0 flex-1">
          <span className="flex min-w-0 items-center gap-1.5">
            <span className="truncate font-bold">{s.name}</span>
            <span className="shrink-0 text-muted-foreground" title={t(s.discoverable ? "instancesettings.servers.listed" : "instancesettings.servers.hidden")}>
              {s.discoverable ? <GlobeIcon className="size-3.5" /> : <EyeOffIcon className="size-3.5" />}
            </span>
            <AnimatePresence mode="popLayout" initial={false}>
              {hasRegions(regions) && (
                <motion.span
                  key={s.region}
                  initial={{ opacity: 0, scale: 0.6, y: -6 }}
                  animate={{ opacity: 1, scale: 1, y: 0 }}
                  exit={{ opacity: 0, scale: 0.6, y: 6 }}
                  transition={SPRING}
                  title={t("instancesettings.servers.keptIn", { region: regionName(regions, s.region) })}
                  className="flex shrink-0 items-center gap-0.5 rounded-full bg-muted px-1.5 py-px text-[0.65rem] font-bold text-muted-foreground"
                >
                  <MapPinIcon className="size-2.5" /> {regionName(regions, s.region)}
                </motion.span>
              )}
            </AnimatePresence>
            <AnimatePresence initial={false}>
              {featured?.includes(s.id) && (
                <motion.span
                  initial={{ opacity: 0, scale: 0.4, rotate: -45 }}
                  animate={{ opacity: 1, scale: 1, rotate: 0 }}
                  exit={{ opacity: 0, scale: 0.4, rotate: 45 }}
                  transition={SPRING}
                  title={t("instancesettings.servers.featured")}
                  className="flex shrink-0 items-center gap-0.5 rounded-full bg-amber-500/15 px-1.5 py-px text-[0.65rem] font-bold text-amber-600 uppercase dark:text-amber-400"
                >
                  <StarIcon className="size-2.5 fill-current" /> {t("instancesettings.servers.featured")}
                </motion.span>
              )}
            </AnimatePresence>
            {entry.member && <span className="shrink-0 rounded-full bg-primary/15 px-1.5 py-px text-[0.65rem] font-bold text-primary uppercase">{t("instancesettings.servers.youreIn")}</span>}
          </span>
          <span className="block truncate text-xs text-muted-foreground">
            {entry.owner
              ? t("instancesettings.servers.byline", { name: displayName(entry.owner), username: entry.owner.username, day: formatDay(toDate(s.createdAt)).toLowerCase() })
              : t("instancesettings.servers.bylineNobody", { day: formatDay(toDate(s.createdAt)).toLowerCase() })}
          </span>
        </span>
        <span className="hidden shrink-0 flex-col items-end text-xs text-muted-foreground tabular-nums sm:flex">
          <span className="flex items-center gap-1">
            <UsersIcon className="size-3" /> {lang.number(membersOf(entry))}
          </span>
          <span className={cn("flex items-center gap-1", full && "font-bold text-amber-500")}>
            <HardDriveIcon className="size-3" /> {formatBytes(lang, storage)}
            {cap !== null && <span className="text-muted-foreground/70">/ {formatBytes(lang, cap)}</span>}
          </span>
        </span>
        <motion.span animate={{ rotate: open ? 180 : 0 }} transition={SPRING} className="shrink-0 text-muted-foreground">
          <ChevronDownIcon className="size-4" />
        </motion.span>
        <span className="absolute inset-x-3 bottom-0 h-0.5 overflow-hidden rounded-full bg-muted/60">
          <motion.span
            className={cn("block h-full rounded-full", full ? "bg-amber-500" : "bg-primary/70")}
            initial={{ x: "-100%" }}
            animate={{ x: `${Math.max(2, share * 100) - 100}%` }}
            transition={{ duration: 0.9, ease: [0.22, 1, 0.36, 1], delay: 0.1 + Math.min(index, 14) * 0.03 }}
          />
        </span>
      </button>
      <AnimatePresence mode="popLayout" initial={false}>
        {open && (
          <motion.div {...SLIDE_IN} transition={SPRING}>
            <Details instanceKey={instanceKey} entry={entry} regions={regions} featured={featured} onFeatured={onFeatured} onMoved={onMoved} onLimits={onLimits} onDeleted={onDeleted} onLeave={onLeave} />
          </motion.div>
        )}
      </AnimatePresence>
    </motion.li>
  );
}

/** A server opened up: what it holds, its caps, and what an admin can do with it. */
function Details({
  instanceKey,
  entry,
  regions,
  featured,
  onFeatured,
  onMoved,
  onLimits,
  onDeleted,
  onLeave,
}: {
  instanceKey: string;
  entry: InstanceServer;
  regions: Region[];
  featured: string[] | null;
  onFeatured: (ids: string[]) => void;
  onMoved: (server: NonNullable<InstanceServer["server"]>) => void;
  onLimits: (limits: ServerLimits) => void;
  onDeleted: () => void;
  onLeave: () => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const s = entry.server!;
  const navigate = useNavigate();
  const [own, setOwn] = useState<Caps | null>(null);
  const [draft, setDraft] = useState<Caps>({});
  const [defaults, setDefaults] = useState<Caps>({});
  const [deleting, setDeleting] = useState(false);
  const save = useAction(setServerLimits);
  const u = entry.usage;

  useEffect(() => {
    Promise.all([run(serverUsage(instanceKey, s.id)), run(nodeUsage(instanceKey))]).then(
      ([usage, node]) => {
        setOwn(caps(usage.ownLimits));
        setDraft(caps(usage.ownLimits));
        setDefaults(caps(node.defaultLimits));
      },
      (e: FuwaError) => toast(e.message),
    );
  }, [instanceKey, s.id]);

  const changed = own ? CAP_FIELDS.filter((f) => draft[f] !== own[f]).length : 0;
  const fallback = (field: (typeof CAP_FIELDS)[number], bytes = false) => {
    const d = defaults[field];
    return t("instancesettings.servers.defaultIs", { value: d === undefined ? t("instancesettings.shared.noLimit") : bytes ? formatBytes(lang, Number(d)) : lang.number(Number(d)) });
  };
  const set = (field: (typeof CAP_FIELDS)[number]) => (value: bigint | undefined) => {
    setDraft((d) => ({ ...d, [field]: value }));
    save.setError(null);
  };

  async function saveCaps() {
    const limits = await save.go(instanceKey, s.id, draft);
    if (!limits) return;
    setOwn(draft);
    onLimits(limits);
    toast(t("instancesettings.servers.capsSaved", { server: s.name }));
  }

  const stats = [
    { label: t("serversettings.nav.members"), value: Number(u?.members ?? 0n) },
    { label: t("serversettings.nav.channels"), value: Number(u?.channels ?? 0n) },
    { label: t("serversettings.usage.messages"), value: Number(u?.messages ?? 0n) },
    { label: t("serversettings.limits.files"), value: Number(u?.attachmentBytes ?? 0n), bytes: true },
  ];

  return (
    <div className="flex flex-col gap-4 border-t border-border/60 p-3 sm:p-4">
      {s.description && <p className="text-sm text-muted-foreground">{s.description}</p>}
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        {stats.map((st, n) => (
          <motion.div
            key={st.label}
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ ...SPRING, delay: 0.05 + n * 0.04 }}
            className="rounded-xl bg-muted/50 px-3 py-2"
          >
            <p className="text-[0.65rem] font-bold tracking-wide text-muted-foreground uppercase">{st.label}</p>
            <p className="text-lg font-extrabold tabular-nums">
              <CountUp value={st.value} delay={0.1 + n * 0.04} format={st.bytes ? (v) => formatBytes(lang, Math.round(v)) : undefined} />
            </p>
          </motion.div>
        ))}
      </div>

      <div className="flex flex-col gap-3">
        <p className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("instancesettings.servers.caps")}</p>
        {own ? (
          <>
            <Cap label={t("serversettings.nav.members")} value={draft.members} onChange={set("members")} placeholder={fallback("members")} />
            <Cap label={t("serversettings.nav.channels")} value={draft.channels} onChange={set("channels")} placeholder={fallback("channels")} />
            <Cap label={t("serversettings.usage.storage")} bytes value={draft.storageBytes} onChange={set("storageBytes")} placeholder={fallback("storageBytes", true)} />
            <Cap label={t("serversettings.limits.files")} bytes value={draft.attachmentBytes} onChange={set("attachmentBytes")} placeholder={fallback("attachmentBytes", true)} />
            <Cap label={t("serversettings.nav.emoji")} value={draft.emojis} onChange={set("emojis")} placeholder={fallback("emojis")} />
            <Cap label={t("serversettings.nav.recordings")} bytes value={draft.recordingBytes} onChange={set("recordingBytes")} placeholder={fallback("recordingBytes", true)} />
          </>
        ) : (
          <div className="shimmer h-40 rounded-xl" />
        )}
        {save.error && <p className="text-sm text-destructive first-letter:uppercase">{save.error}</p>}
        <AnimatePresence mode="popLayout" initial={false}>
          {changed > 0 && (
            <motion.div
              {...SLIDE_IN}
              transition={SPRING}
              className="flex justify-end gap-2"
            >
              <Button type="button" variant="ghost" size="sm" className="rounded-xl" disabled={save.pending} onClick={() => own && setDraft(own)}>
                {t("settings.controls.discard")}
              </Button>
              <Button type="button" size="sm" className="btn rounded-xl px-4 font-bold" disabled={save.pending} onClick={saveCaps}>
                {save.pending && <LoaderCircleIcon className="animate-spin" />} {t("instancesettings.servers.saveCaps")}
              </Button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      {featured && <FeatureInBrowse instanceKey={instanceKey} server={s} featured={featured} onFeatured={onFeatured} />}

      {hasRegions(regions) && <MoveRegion instanceKey={instanceKey} server={s} regions={regions} onMoved={onMoved} />}

      <Shares instanceKey={instanceKey} serverId={s.id} />

      <div className="flex flex-wrap items-center gap-2 border-t border-border/60 pt-3">
        {entry.member && (
          <Button
            type="button"
            variant="outline"
            className="group rounded-xl"
            onClick={() => {
              onLeave();
              void navigate({ to: "/$instance/$server", params: { instance: instanceKey, server: s.id } });
            }}
          >
            {t("instancesettings.servers.openIt")} <ArrowRightIcon className="transition-transform group-hover:translate-x-0.5" />
          </Button>
        )}
        <ExportButton instanceKey={instanceKey} serverId={s.id} />
        <Button
          type="button"
          variant="ghost"
          onClick={() => setDeleting((d) => !d)}
          className={cn("group ml-auto rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive", deleting && "bg-destructive/10")}
        >
          <Trash2Icon className="transition-transform group-hover:-rotate-12" /> {t("serversettings.shared.delete")}
        </Button>
      </div>
      <AnimatePresence mode="popLayout" initial={false}>
        {deleting && (
          <motion.div {...SLIDE_IN} transition={SPRING}>
            <DeleteServer instanceKey={instanceKey} name={s.name} serverId={s.id} onDeleted={onDeleted} />
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/**
 * Featuring a server in Browse, where it shows first and big, and its place
 * among the others featured. Every change sends the whole list.
 */
function FeatureInBrowse({
  instanceKey,
  server,
  featured,
  onFeatured,
}: {
  instanceKey: string;
  server: NonNullable<InstanceServer["server"]>;
  featured: string[];
  onFeatured: (ids: string[]) => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const save = useAction(setFeaturedServers);
  const place = featured.indexOf(server.id);
  const on = place >= 0;

  async function change(ids: string[], said?: string) {
    const saved = await save.go(instanceKey, ids);
    if (!saved) return;
    onFeatured(saved);
    if (said) toast(said);
  }
  const swap = (by: number) => {
    const ids = [...featured];
    [ids[place], ids[place + by]] = [ids[place + by]!, ids[place]!];
    void change(ids);
  };

  return (
    <div className="flex flex-col gap-2">
      <label className="flex cursor-pointer items-start gap-3 rounded-xl bg-muted/50 px-3 py-2.5">
        <motion.span animate={{ rotate: on ? 72 : 0, scale: on ? 1.1 : 1 }} transition={SPRING} className={cn("mt-0.5 shrink-0", on ? "text-amber-500" : "text-muted-foreground")}>
          <StarIcon className={cn("size-4", on && "fill-current")} />
        </motion.span>
        <span className="min-w-0 flex-1">
          <span className="block text-sm font-bold">{t("instancesettings.servers.feature")}</span>
          <span className="block text-xs text-muted-foreground">{t("instancesettings.servers.featureAbout")}</span>
          {on && !server.discoverable && <span className="mt-1 block text-xs font-bold text-amber-600 dark:text-amber-400">{t("instancesettings.servers.featureHidden")}</span>}
        </span>
        <Switch
          checked={on}
          disabled={save.pending}
          onCheckedChange={(next) =>
            change(
              next ? [...featured, server.id] : featured.filter((id) => id !== server.id),
              t(next ? "instancesettings.servers.featuredNow" : "instancesettings.servers.unfeaturedNow", { server: server.name }),
            )
          }
          className="mt-0.5"
        />
      </label>
      <AnimatePresence mode="popLayout" initial={false}>
        {on && featured.length > 1 && (
          <motion.div {...SLIDE_IN} transition={SPRING} className="flex items-center gap-2 px-1">
            <span className="flex-1 text-xs text-muted-foreground tabular-nums">
              {t("instancesettings.servers.featurePlace", { place: lang.number(place + 1), count: lang.number(featured.length) })}
            </span>
            <Button type="button" variant="ghost" size="sm" className="group rounded-xl" disabled={save.pending || place === 0} onClick={() => swap(-1)}>
              <ArrowUpIcon className="transition-transform group-hover:-translate-y-0.5" /> {t("instancesettings.servers.featureEarlier")}
            </Button>
            <Button type="button" variant="ghost" size="sm" className="group rounded-xl" disabled={save.pending || place === featured.length - 1} onClick={() => swap(1)}>
              <ArrowDownIcon className="transition-transform group-hover:translate-y-0.5" /> {t("instancesettings.servers.featureLater")}
            </Button>
          </motion.div>
        )}
      </AnimatePresence>
      {save.error && <p className="text-sm text-destructive first-letter:uppercase">{save.error}</p>}
    </div>
  );
}

/**
 * A server's shared channels, both ends, each of which an instance admin can
 * end. Hidden while it has none.
 */
function Shares({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const [shares, setShares] = useState<SharedConnection[] | null>(null);
  const [ending, setEnding] = useState<SharedConnection | null>(null);

  useEffect(() => {
    run(listServerShares(instanceKey, serverId)).then(setShares, (e: FuwaError) => toast(e.message));
  }, [instanceKey, serverId]);

  if (!shares?.length) return null;
  const other = (c: SharedConnection) => c.server?.name ?? t("serversettings.sharedChannels.anotherServer");
  const channel = (c: SharedConnection) => `#${c.homeChannelName || t("instancesettings.servers.aChannel")}`;
  return (
    <div className="flex flex-col gap-2">
      <p className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("serversettings.nav.shared")}</p>
      <ul className="flex flex-col gap-2">
        <AnimatePresence mode="popLayout" initial={false}>
          {shares.map((c, n) => (
            <motion.li
              key={c.id}
              layout
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, x: -16 }}
              transition={{ ...SPRING, delay: n * 0.04 }}
              className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-xl bg-muted/50 px-3 py-2"
            >
              <span className="relative shrink-0">
                {c.server && <ServerIcon server={c.server} className="size-8 rounded-lg" />}
                <span className="absolute -right-1 -bottom-1 grid size-4 place-items-center rounded-full bg-background text-primary">
                  <SharedGlyph className="size-2.5" />
                </span>
              </span>
              <p className="min-w-0 flex-1 basis-40 text-sm">
                <T
                  k={c.home ? "instancesettings.servers.shownIn" : "instancesettings.servers.from"}
                  values={{ channel: <b>{channel(c)}</b>, server: <b>{other(c)}</b> }}
                />
              </p>
              <StateChip connection={c} />
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => setEnding(c)}
                className="group rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive"
              >
                <UnlinkIcon className="transition-transform group-hover:-rotate-12" /> {t("instancesettings.servers.end")}
              </Button>
            </motion.li>
          ))}
        </AnimatePresence>
      </ul>
      <ConfirmDialog
        open={ending !== null}
        onOpenChange={(open) => !open && setEnding(null)}
        title={
          ending
            ? t(ending.state === SharedConnectionState.WAITING ? "instancesettings.servers.endRequestAsk" : "instancesettings.servers.endSharingAsk", { channel: channel(ending) })
            : ""
        }
        body={ending ? t(ending.home ? "instancesettings.servers.endHomeBody" : "instancesettings.servers.endGuestBody", { server: other(ending) }) : ""}
        action={t("instancesettings.servers.endIt")}
        onConfirm={async () => {
          if (!ending) return;
          await run(endServerShare(instanceKey, serverId, ending.id));
          setShares((list) => list?.filter((x) => x.id !== ending.id) ?? null);
          toast(t("instancesettings.servers.ended", { channel: channel(ending), server: other(ending) }));
        }}
      />
    </div>
  );
}

/**
 * Moves a server to another region: pick one, confirm, and it travels there.
 * Changes wait for the moment it takes; its calls drop and people rejoin.
 */
function MoveRegion({
  instanceKey,
  server,
  regions,
  onMoved,
}: {
  instanceKey: string;
  server: NonNullable<InstanceServer["server"]>;
  regions: Region[];
  onMoved: (server: NonNullable<InstanceServer["server"]>) => void;
}) {
  const { t } = useI18n();
  const [to, setTo] = useState<string | null>(null);
  const move = useAction(moveServer);
  const here = regionName(regions, server.region);
  const target = to === null ? null : regions.find((r) => r.id === to);

  async function go() {
    if (!target) return;
    const moved = await move.go(instanceKey, server.id, target.id);
    if (!moved) return;
    setTo(null);
    onMoved(moved);
    toast(t("instancesettings.servers.movedTo", { server: server.name, region: target.name }));
  }

  return (
    <div className="flex flex-col gap-3">
      <p className="flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
        <MapPinIcon className="size-3.5" /> {t("instancesettings.servers.region")}
      </p>
      <div role="radiogroup" aria-label={t("instancesettings.servers.regionFor", { server: server.name })} className="flex flex-wrap gap-2">
        {regions.map((r) => {
          const current = sameRegion(regions, r.id, server.region);
          const on = current ? to === null : to === r.id;
          return (
            <button
              key={r.id}
              type="button"
              role="radio"
              aria-checked={on}
              disabled={move.pending}
              onClick={() => {
                setTo(current ? null : r.id);
                move.setError(null);
              }}
              className={cn(
                "relative flex items-center gap-2 rounded-full border py-1.5 pr-3 pl-1.5 text-sm font-bold transition-colors disabled:opacity-60",
                on ? "border-primary text-primary" : "hover:border-primary/50",
              )}
            >
              {on && <motion.span layoutId={`move-region-${server.id}`} transition={SPRING} className="absolute inset-0 rounded-full bg-primary/15" />}
              <span className={cn("relative grid size-6 place-items-center rounded-full text-[0.6rem] font-extrabold", on ? "bg-primary text-primary-foreground" : "bg-muted text-muted-foreground")}>
                {regionMark(r.name)}
              </span>
              <span className="relative">{r.name}</span>
              {current && <span className="relative text-xs font-normal text-muted-foreground">{t("instancesettings.servers.now")}</span>}
            </button>
          );
        })}
      </div>
      <AnimatePresence mode="popLayout" initial={false}>
        {target && (
          <motion.div {...SLIDE_IN} transition={SPRING}>
            <div className="flex flex-col gap-3 rounded-2xl border border-primary/30 bg-primary/5 p-4">
              <div className="flex items-center gap-2 text-sm font-bold">
                <span className="truncate">{here}</span>
                <span className="relative h-px min-w-12 flex-1 bg-border">
                  <motion.span
                    className="absolute inset-0"
                    initial={{ x: "0%" }}
                    animate={move.pending ? { x: ["0%", "88%"], opacity: [0, 1, 1, 0] } : { x: "44%" }}
                    transition={move.pending ? { duration: 1.1, repeat: Infinity, ease: "easeInOut" } : SPRING}
                  >
                    <PlaneIcon className="absolute top-1/2 left-0 size-4 -translate-y-1/2 text-primary" />
                  </motion.span>
                </span>
                <span className="truncate text-primary">{target.name}</span>
              </div>
              <ul className="flex flex-col gap-1 text-sm text-muted-foreground">
                <li>{t("instancesettings.servers.moveWhat", { to: target.name, from: here })}</li>
                <li>{t("instancesettings.servers.moveWait")}</li>
              </ul>
              {move.error && <p className="text-sm text-destructive first-letter:uppercase">{move.error}</p>}
              <div className="flex justify-end gap-2">
                <Button type="button" variant="ghost" size="sm" className="rounded-xl" disabled={move.pending} onClick={() => setTo(null)}>
                  {t("common.cancel")}
                </Button>
                <Button type="button" size="sm" className="btn group rounded-xl px-4 font-bold" disabled={move.pending} onClick={go}>
                  {move.pending ? <LoaderCircleIcon className="animate-spin" /> : <PlaneIcon className="transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5" />}
                  {t(move.pending ? "instancesettings.servers.moving" : "instancesettings.servers.moveTo", { region: target.name })}
                </Button>
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** Saves the server's whole database as one SQLite file, filling up as it arrives. */
function ExportButton({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const lang = useI18n();
  const [progress, setProgress] = useState<number | null>(null);
  const [done, setDone] = useState(false);

  async function go() {
    setProgress(0);
    setDone(false);
    try {
      const { blob, filename } = await run(exportServer(instanceKey, serverId, (bytes, total) => setProgress(total ? bytes / total : 0.5)));
      const stamped = filename.replace(/\.db$/, `-${new Date().toISOString().slice(0, 10)}.db`);
      const url = URL.createObjectURL(blob);
      Object.assign(document.createElement("a"), { href: url, download: stamped }).click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      setDone(true);
      toast(lang.t("instancesettings.servers.savedFile", { file: stamped, size: formatBytes(lang, blob.size) }));
      setTimeout(() => setDone(false), 2200);
    } catch (e) {
      toast((e as FuwaError).message);
    } finally {
      setProgress(null);
    }
  }

  const working = progress !== null;
  return (
    <Button type="button" variant="outline" onClick={go} disabled={working} className="group relative overflow-hidden rounded-xl">
      <AnimatePresence>
        {working && (
          <motion.span
            className="absolute inset-0 bg-primary/20"
            initial={{ x: "-100%" }}
            animate={{ x: `${Math.max(4, progress * 100) - 100}%` }}
            exit={{ opacity: 0 }}
            transition={SPRING}
          />
        )}
      </AnimatePresence>
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={done ? "done" : working ? "working" : "idle"}
          initial={{ scale: 0, y: -6 }}
          animate={{ scale: 1, y: 0 }}
          exit={{ scale: 0, y: 6 }}
          transition={SPRING}
          className="relative inline-flex"
        >
          {done ? (
            <CheckIcon className="text-emerald-500" />
          ) : working ? (
            <LoaderCircleIcon className="animate-spin" />
          ) : (
            <DownloadIcon className="transition-transform group-hover:translate-y-0.5" />
          )}
        </motion.span>
      </AnimatePresence>
      <span className="relative tabular-nums">{working
          ? lang.t("instancesettings.servers.saving", { percent: lang.number(Math.round(progress * 100) / 100, { style: "percent" }) })
          : lang.t("instancesettings.servers.saveFile")}</span>
    </Button>
  );
}

function DeleteServer({ instanceKey, name, serverId, onDeleted }: { instanceKey: string; name: string; serverId: string; onDeleted: () => void }) {
  const { t } = useI18n();
  const [confirm, setConfirm] = useState("");
  const remove = useAction(deleteServer);
  const armed = confirm === name;
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!armed || (await remove.go(instanceKey, serverId)) === undefined) return;
    toast(t("serversettings.shared.deleted", { name }));
    onDeleted();
  }
  return (
    <form onSubmit={submit} className="flex flex-col gap-3 rounded-2xl border border-destructive/40 bg-destructive/5 p-4">
      <p className="flex items-center gap-2 font-bold text-destructive">
        <TriangleAlertIcon className="size-4" /> {t("serversettings.danger.title", { server: name })}
      </p>
      <p className="text-sm text-muted-foreground">{t("instancesettings.servers.deleteHint")}</p>
      <Label htmlFor={`confirm-delete-${serverId}`} className="text-sm">
        <T k="serversettings.danger.confirm" values={{ name: <b>{name}</b> }} />
      </Label>
      <Input
        id={`confirm-delete-${serverId}`}
        value={confirm}
        onChange={(e) => setConfirm(e.target.value)}
        className={cn("h-10 rounded-xl transition-colors", armed && "border-destructive ring-2 ring-destructive/20")}
        autoComplete="off"
      />
      {remove.error && <p className="text-sm text-destructive first-letter:uppercase">{remove.error}</p>}
      <motion.div className="self-end" initial={false} animate={armed ? { scale: [1, 1.08, 1], rotate: [0, -2, 2, 0] } : { scale: 1, rotate: 0 }} transition={{ duration: 0.4 }}>
        <Button type="submit" variant="destructive" disabled={!armed || remove.pending} className="rounded-xl font-bold">
          {remove.pending ? <LoaderCircleIcon className="animate-spin" /> : <Trash2Icon />} {t("serversettings.nav.danger")}
        </Button>
      </motion.div>
    </form>
  );
}
