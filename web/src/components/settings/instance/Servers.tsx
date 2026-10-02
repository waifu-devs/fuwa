import { useNavigate } from "@tanstack/react-router";
import {
  ArrowRightIcon,
  CheckIcon,
  ChevronDownIcon,
  DownloadIcon,
  EyeOffIcon,
  GlobeIcon,
  HardDriveIcon,
  ImageIcon,
  LoaderCircleIcon,
  SearchIcon,
  ServerIcon as ServersIcon,
  Trash2Icon,
  TriangleAlertIcon,
  UsersIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import type { InstanceServer } from "@/gen/fuwa/v1/admin_pb";
import type { ServerLimits } from "@/gen/fuwa/v1/types_pb";
import { deleteServer, exportServer, listInstanceServers, nodeUsage, run, serverUsage, setServerLimits } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction } from "@/fuwa/hooks";
import { ServerIcon } from "@/components/Icons";
import { Count, CountUp, SPRING } from "@/components/motion";
import { Segmented } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { displayName, formatBytes, formatDay, toDate } from "@/lib/format";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { Cap } from "../controls";

type Sort = "storage" | "members" | "newest";
type Caps = Omit<ServerLimits, "$typeName">;
const CAP_FIELDS = ["members", "channels", "storageBytes", "attachmentBytes", "emojis"] as const;
const caps = (l: ServerLimits | undefined): Caps => ({
  members: l?.members,
  channels: l?.channels,
  storageBytes: l?.storageBytes,
  attachmentBytes: l?.attachmentBytes,
  emojis: l?.emojis,
});

const storageOf = (s: InstanceServer) => Number(s.usage?.storageBytes ?? 0n);
const membersOf = (s: InstanceServer) => Number(s.usage?.members ?? s.server?.memberCount ?? 0n);

/**
 * Every community server on the instance, with its owner and what it holds.
 * Admins can change a server's caps, save its whole file, or delete it,
 * whether or not they're in it.
 */
export function Servers({ instanceKey, onLeave }: { instanceKey: string; onLeave: () => void }) {
  const [servers, setServers] = useState<InstanceServer[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<Sort>("storage");
  const [open, setOpen] = useState<string | null>(null);
  const [pictures, setPictures] = useState({ count: 0, bytes: 0 });

  useEffect(() => {
    run(listInstanceServers(instanceKey)).then(setServers, (e: FuwaError) => setError(e.message));
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
    { label: "Servers", value: servers?.length ?? 0, icon: ServersIcon },
    { label: "Memberships", value: (servers ?? []).reduce((n, s) => n + membersOf(s), 0), icon: UsersIcon },
    { label: "Storage", value: (servers ?? []).reduce((n, s) => n + storageOf(s), 0), icon: HardDriveIcon, bytes: true },
    { label: "Pictures", value: pictures.bytes, icon: ImageIcon, bytes: true, sub: `${pictures.count.toLocaleString()} avatars, banners and icons` },
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
        {totals.map((t, n) => (
          <motion.div
            key={t.label}
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ ...SPRING, delay: n * 0.05 }}
            title={t.sub}
            className="rounded-2xl border bg-background/50 p-3"
          >
            <p className="flex items-center gap-1.5 text-[0.65rem] font-bold tracking-wide text-muted-foreground uppercase">
              <t.icon className="size-3.5" /> {t.label}
            </p>
            <p className="mt-1 truncate text-xl font-extrabold tabular-nums">
              <CountUp value={t.value} delay={0.1 + n * 0.05} format={t.bytes ? (v) => formatBytes(Math.round(v)) : undefined} />
            </p>
          </motion.div>
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-0 flex-1 basis-56">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search servers or owners" aria-label="Search servers" className="h-10 rounded-xl pl-9" />
        </div>
        <Segmented
          label="Sort by"
          value={sort}
          onChange={setSort}
          options={[
            { value: "storage", label: "Biggest" },
            { value: "members", label: "Members" },
            { value: "newest", label: "Newest" },
          ]}
        />
      </div>

      <p className="flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
        <ServersIcon className="size-3.5" /> <Count value={shown.length} /> {shown.length === 1 ? "server" : "servers"}
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
            {query ? "No server matches that." : "Nobody has made a server here yet."}
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
  onToggle,
  onLimits,
  onDeleted,
  onLeave,
}: {
  instanceKey: string;
  entry: InstanceServer;
  index: number;
  biggest: number;
  open: boolean;
  onToggle: () => void;
  onLimits: (limits: ServerLimits) => void;
  onDeleted: () => void;
  onLeave: () => void;
}) {
  const s = entry.server!;
  const storage = storageOf(entry);
  const cap = entry.limits?.storageBytes === undefined ? null : Number(entry.limits.storageBytes);
  const share = cap ? Math.min(1, storage / cap) : storage / biggest;
  const full = cap !== null && share >= 0.9;
  return (
    <motion.li
      layout
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
            <span className="shrink-0 text-muted-foreground" title={s.discoverable ? "Listed in Browse" : "Hidden from Browse"}>
              {s.discoverable ? <GlobeIcon className="size-3.5" /> : <EyeOffIcon className="size-3.5" />}
            </span>
            {entry.member && <span className="shrink-0 rounded-full bg-primary/15 px-1.5 py-px text-[0.65rem] font-bold text-primary uppercase">You're in it</span>}
          </span>
          <span className="block truncate text-xs text-muted-foreground">
            by {entry.owner ? `${displayName(entry.owner)} (@${entry.owner.username})` : "nobody"} · made {formatDay(toDate(s.createdAt)).toLowerCase()}
          </span>
        </span>
        <span className="hidden shrink-0 flex-col items-end text-xs text-muted-foreground tabular-nums sm:flex">
          <span className="flex items-center gap-1">
            <UsersIcon className="size-3" /> {membersOf(entry).toLocaleString()}
          </span>
          <span className={cn("flex items-center gap-1", full && "font-bold text-amber-500")}>
            <HardDriveIcon className="size-3" /> {formatBytes(storage)}
            {cap !== null && <span className="text-muted-foreground/70">/ {formatBytes(cap)}</span>}
          </span>
        </span>
        <motion.span animate={{ rotate: open ? 180 : 0 }} transition={SPRING} className="shrink-0 text-muted-foreground">
          <ChevronDownIcon className="size-4" />
        </motion.span>
        <span className="absolute inset-x-3 bottom-0 h-0.5 overflow-hidden rounded-full bg-muted/60">
          <motion.span
            className={cn("block h-full rounded-full", full ? "bg-amber-500" : "bg-primary/70")}
            initial={{ width: 0 }}
            animate={{ width: `${Math.max(2, share * 100)}%` }}
            transition={{ duration: 0.9, ease: [0.22, 1, 0.36, 1], delay: 0.1 + Math.min(index, 14) * 0.03 }}
          />
        </span>
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={SPRING}
            className="overflow-hidden"
          >
            <Details instanceKey={instanceKey} entry={entry} onLimits={onLimits} onDeleted={onDeleted} onLeave={onLeave} />
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
  onLimits,
  onDeleted,
  onLeave,
}: {
  instanceKey: string;
  entry: InstanceServer;
  onLimits: (limits: ServerLimits) => void;
  onDeleted: () => void;
  onLeave: () => void;
}) {
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
    return `Default: ${d === undefined ? "no limit" : bytes ? formatBytes(Number(d)) : Number(d).toLocaleString()}`;
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
    toast(`Saved ${s.name}'s caps`);
  }

  const stats = [
    { label: "Members", value: Number(u?.members ?? 0n) },
    { label: "Channels", value: Number(u?.channels ?? 0n) },
    { label: "Messages", value: Number(u?.messages ?? 0n) },
    { label: "Files", value: Number(u?.attachmentBytes ?? 0n), bytes: true },
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
              <CountUp value={st.value} delay={0.1 + n * 0.04} format={st.bytes ? (v) => formatBytes(Math.round(v)) : undefined} />
            </p>
          </motion.div>
        ))}
      </div>

      <div className="flex flex-col gap-3">
        <p className="text-xs font-bold tracking-wide text-muted-foreground uppercase">Caps for this server</p>
        {own ? (
          <>
            <Cap label="Members" value={draft.members} onChange={set("members")} placeholder={fallback("members")} />
            <Cap label="Channels" value={draft.channels} onChange={set("channels")} placeholder={fallback("channels")} />
            <Cap label="Storage" bytes value={draft.storageBytes} onChange={set("storageBytes")} placeholder={fallback("storageBytes", true)} />
            <Cap label="Files" bytes value={draft.attachmentBytes} onChange={set("attachmentBytes")} placeholder={fallback("attachmentBytes", true)} />
            <Cap label="Emoji" value={draft.emojis} onChange={set("emojis")} placeholder={fallback("emojis")} />
          </>
        ) : (
          <div className="shimmer h-40 rounded-xl" />
        )}
        {save.error && <p className="text-sm text-destructive first-letter:uppercase">{save.error}</p>}
        <AnimatePresence initial={false}>
          {changed > 0 && (
            <motion.div
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: "auto" }}
              exit={{ opacity: 0, height: 0 }}
              transition={SPRING}
              className="flex justify-end gap-2 overflow-hidden"
            >
              <Button type="button" variant="ghost" size="sm" className="rounded-xl" disabled={save.pending} onClick={() => own && setDraft(own)}>
                Discard
              </Button>
              <Button type="button" size="sm" className="btn rounded-xl px-4 font-bold" disabled={save.pending} onClick={saveCaps}>
                {save.pending && <LoaderCircleIcon className="animate-spin" />} Save caps
              </Button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>

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
            Open it <ArrowRightIcon className="transition-transform group-hover:translate-x-0.5" />
          </Button>
        )}
        <ExportButton instanceKey={instanceKey} serverId={s.id} />
        <Button
          type="button"
          variant="ghost"
          onClick={() => setDeleting((d) => !d)}
          className={cn("group ml-auto rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive", deleting && "bg-destructive/10")}
        >
          <Trash2Icon className="transition-transform group-hover:-rotate-12" /> Delete
        </Button>
      </div>
      <AnimatePresence initial={false}>
        {deleting && (
          <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
            <DeleteServer instanceKey={instanceKey} name={s.name} serverId={s.id} onDeleted={onDeleted} />
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** Saves the server's whole database as one SQLite file, filling up as it arrives. */
function ExportButton({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
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
      toast(`Saved ${stamped} (${formatBytes(blob.size)})`);
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
            className="absolute inset-y-0 left-0 bg-primary/20"
            initial={{ width: 0 }}
            animate={{ width: `${Math.max(4, progress * 100)}%` }}
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
      <span className="relative tabular-nums">{working ? `Saving ${Math.round(progress * 100)}%` : "Save its file"}</span>
    </Button>
  );
}

function DeleteServer({ instanceKey, name, serverId, onDeleted }: { instanceKey: string; name: string; serverId: string; onDeleted: () => void }) {
  const [confirm, setConfirm] = useState("");
  const remove = useAction(deleteServer);
  const armed = confirm === name;
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!armed || (await remove.go(instanceKey, serverId)) === undefined) return;
    toast(`Deleted ${name}`);
    onDeleted();
  }
  return (
    <form onSubmit={submit} className="flex flex-col gap-3 rounded-2xl border border-destructive/40 bg-destructive/5 p-4">
      <p className="flex items-center gap-2 font-bold text-destructive">
        <TriangleAlertIcon className="size-4" /> Delete {name}
      </p>
      <p className="text-sm text-muted-foreground">Everyone loses its channels and messages. Save its file first if you might want it back.</p>
      <Label htmlFor={`confirm-delete-${serverId}`} className="text-sm">
        Type <b>{name}</b> to confirm
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
          {remove.pending ? <LoaderCircleIcon className="animate-spin" /> : <Trash2Icon />} Delete server
        </Button>
      </motion.div>
    </form>
  );
}
