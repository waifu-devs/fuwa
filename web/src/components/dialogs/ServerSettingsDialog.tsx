import { useNavigate } from "@tanstack/react-router";
import { ChartColumnIcon, EyeOffIcon, GaugeIcon, LoaderCircleIcon, SettingsIcon, Trash2Icon, TriangleAlertIcon, UsersIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import type { GetServerUsageResponse } from "@/gen/fuwa/v1/server_pb";
import type { Server, ServerLimits } from "@/gen/fuwa/v1/types_pb";
import { deleteServer, nodeUsage, run, serverUsage, setServerLimits, updateServer } from "@/fuwa/actions";
import { useAction } from "@/fuwa/hooks";
import { ServerIcon } from "@/components/Icons";
import { InlineMarkdown } from "@/components/Markdown";
import { Count, CountUp, SPRING, SwapText } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { formatBytes, initials } from "@/lib/format";
import { cn } from "@/lib/utils";
import { Cap, SaveBar, WithPreview } from "@/components/settings/controls";
import { SettingsScreen } from "@/components/settings/SettingsScreen";

export function ServerSettingsDialog({
  open,
  onOpenChange,
  instanceKey,
  server,
  isOwner,
  instanceAdmin = false,
  tab: initialTab = "overview",
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  server: Server;
  isOwner: boolean;
  /** Instance admins also get the server's own caps. */
  instanceAdmin?: boolean;
  tab?: string;
}) {
  const [tab, setTab] = useState(initialTab);
  useEffect(() => {
    if (open) setTab(initialTab);
  }, [open, initialTab]);
  const sections = [
    { id: "overview", label: "Overview", icon: SettingsIcon, description: "How the server looks and whether people can find it." },
    { id: "usage", label: "Usage", icon: ChartColumnIcon, description: "What the server holds, against its caps." },
    ...(instanceAdmin
      ? [{ id: "limits", label: "Limits", icon: GaugeIcon, description: "Caps for this server only, over the instance's defaults." }]
      : []),
  ];
  return (
    <SettingsScreen
      open={open}
      onOpenChange={onOpenChange}
      title={server.name}
      subtitle="Server settings"
      section={tab}
      onSectionChange={setTab}
      openToSection={initialTab !== "overview"}
      groups={[
        { label: server.name, sections },
        ...(isOwner ? [{ sections: [{ id: "danger", label: "Delete server", icon: Trash2Icon, danger: true }] }] : []),
      ]}
    >
      {tab === "overview" && <Overview instanceKey={instanceKey} server={server} />}
      {tab === "usage" && <Usage instanceKey={instanceKey} serverId={server.id} />}
      {tab === "limits" && instanceAdmin && <Limits instanceKey={instanceKey} serverId={server.id} />}
      {tab === "danger" && isOwner && <Danger instanceKey={instanceKey} server={server} onDeleted={() => onOpenChange(false)} />}
    </SettingsScreen>
  );
}

function Overview({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const [name, setName] = useState(server.name);
  const [description, setDescription] = useState(server.description);
  const [discoverable, setDiscoverable] = useState(server.discoverable);
  const save = useAction(updateServer);
  const changes = [name !== server.name, description !== server.description, discoverable !== server.discoverable].filter(Boolean).length;

  function discard() {
    setName(server.name);
    setDescription(server.description);
    setDiscoverable(server.discoverable);
    save.setError(null);
  }

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    if (!name.trim()) return save.setError("a server needs a name");
    await save.go(instanceKey, server.id, {
      ...(name !== server.name && { name: name.trim() }),
      ...(description !== server.description && { description: description.trim() }),
      ...(discoverable !== server.discoverable && { discoverable }),
    });
  }

  const shown = { ...server, name: name || server.name, description, discoverable };
  return (
    <form onSubmit={submit}>
      <WithPreview preview={<BrowseCard server={shown} />}>
        <div className="flex flex-col">
          <div className="flex items-center gap-4 border-b border-border/70 pb-5">
            <motion.span key={initials(shown.name)} initial={{ scale: 0.85, rotate: -8 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 600, damping: 16 }}>
              <ServerIcon server={shown} active className="size-20 text-2xl" />
            </motion.span>
            <div className="flex min-w-0 flex-1 flex-col gap-2">
              <Label htmlFor="settings-name" className="font-extrabold">
                Name
              </Label>
              <Input id="settings-name" required maxLength={100} value={name} onChange={(e) => setName(e.target.value)} className="h-11 rounded-xl" />
            </div>
          </div>
          <div className="flex flex-col gap-2 border-b border-border/70 py-5">
            <Label htmlFor="settings-description" className="font-extrabold">
              Description
            </Label>
            <Textarea id="settings-description" rows={4} maxLength={1000} value={description} onChange={(e) => setDescription(e.target.value)} className="rounded-xl" />
            <p className="text-sm text-muted-foreground">Shown in Browse. Markdown works.</p>
          </div>
          <label className="flex cursor-pointer items-center justify-between gap-4 py-5">
            <span>
              <span className="block font-extrabold">Show in Browse</span>
              <span className="block text-sm text-muted-foreground">Anyone on this fuwa server can find and join it.</span>
            </span>
            <Switch checked={discoverable} onCheckedChange={setDiscoverable} />
          </label>
        </div>
        <SaveBar count={changes} saving={save.pending} error={save.error} onSave={() => void submit()} onDiscard={discard} />
      </WithPreview>
    </form>
  );
}

/** The server as people find it in Browse, or hidden from it. */
function BrowseCard({ server }: { server: Server }) {
  return (
    <div className="relative overflow-hidden rounded-3xl border bg-card shadow-lg">
      <motion.div animate={{ opacity: server.discoverable ? 1 : 0.2, filter: server.discoverable ? "blur(0px)" : "blur(3px)" }} transition={{ duration: 0.3 }} className="flex flex-col gap-3 p-5">
        <div className="flex items-center gap-3">
          <ServerIcon server={server} active className="size-14 text-lg" />
          <div className="min-w-0">
            <p className="truncate text-lg font-extrabold">
              <SwapText className="truncate align-bottom">{server.name}</SwapText>
            </p>
            <p className="flex items-center gap-1 text-xs text-muted-foreground">
              <UsersIcon className="size-3.5" /> <Count value={Number(server.memberCount)} /> {server.memberCount === 1n ? "member" : "members"}
            </p>
          </div>
        </div>
        <p className="line-clamp-4 text-sm break-words text-muted-foreground">
          {server.description.trim() ? <InlineMarkdown>{server.description}</InlineMarkdown> : "No description yet."}
        </p>
        <span className="btn grid h-9 place-items-center rounded-xl bg-primary text-sm font-bold text-primary-foreground">Join</span>
      </motion.div>
      <AnimatePresence>
        {!server.discoverable && (
          <motion.div
            initial={{ opacity: 0, scale: 0.9 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.9 }}
            transition={SPRING}
            className="absolute inset-0 grid place-items-center p-6 text-center"
          >
            <span className="flex flex-col items-center gap-1.5">
              <EyeOffIcon className="size-6 text-muted-foreground" />
              <span className="text-sm font-extrabold">Hidden from Browse</span>
              <span className="text-xs text-muted-foreground">Only its members see it.</span>
            </span>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function Usage({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const [data, setData] = useState<GetServerUsageResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    run(serverUsage(instanceKey, serverId)).then(setData, (e) => setError(e.message));
  }, [instanceKey, serverId]);

  if (error) return <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>;
  if (!data?.usage) return <div className="grid gap-3 sm:grid-cols-2">{[0, 1, 2, 3].map((n) => <div key={n} className="shimmer h-24 rounded-2xl" />)}</div>;
  const u = data.usage;
  const l = data.limits;
  const rows = [
    { label: "Members", value: Number(u.members), limit: l?.members },
    { label: "Channels", value: Number(u.channels), limit: l?.channels },
    { label: "Messages", value: Number(u.messages), sub: `${Number(u.messagesSent).toLocaleString()} sent all time` },
    { label: "Storage", value: Number(u.storageBytes), limit: l?.storageBytes, bytes: true },
    { label: "Attachments", value: Number(u.attachmentBytes), limit: l?.attachmentBytes, bytes: true, sub: `${Number(u.attachments)} files` },
    { label: "Events", value: Number(u.events), sub: "in the server's log" },
  ];
  return (
    <div className="flex flex-col gap-3">
      <div className="grid gap-3 sm:grid-cols-2">
        {rows.map((r, n) => {
          const limit = r.limit === undefined ? null : Number(r.limit);
          const share = limit ? Math.min(1, r.value / limit) : 0;
          return (
            <motion.div
              key={r.label}
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: n * 0.05 }}
              className="rounded-2xl border bg-background/50 p-4"
            >
              <p className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{r.label}</p>
              <p className="mt-1 text-2xl font-extrabold tabular-nums">
                <CountUp value={r.value} delay={0.1 + n * 0.05} format={r.bytes ? (v) => formatBytes(Math.round(v)) : undefined} />
              </p>
              <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-muted">
                <motion.div
                  className="h-full rounded-full bg-primary"
                  initial={{ width: 0 }}
                  animate={{ width: limit ? `${share * 100}%` : "100%", opacity: limit ? 1 : 0.25 }}
                  transition={{ duration: 0.9, ease: [0.22, 1, 0.36, 1], delay: 0.1 + n * 0.05 }}
                />
              </div>
              <p className="mt-1.5 text-xs text-muted-foreground">
                {limit !== null ? `of ${r.bytes ? formatBytes(limit) : limit.toLocaleString()}` : (r.sub ?? "no limit")}
              </p>
            </motion.div>
          );
        })}
      </div>
      <p className="text-xs text-muted-foreground">Limits are set by whoever runs this fuwa server. Self-hosted servers have none unless the operator adds them.</p>
    </div>
  );
}

type Caps = Omit<ServerLimits, "$typeName">;
const CAP_FIELDS = ["members", "channels", "storageBytes", "attachmentBytes"] as const;
const caps = (l: ServerLimits | undefined): Caps => ({
  members: l?.members,
  channels: l?.channels,
  storageBytes: l?.storageBytes,
  attachmentBytes: l?.attachmentBytes,
});

/** Instance admins: this server's own caps, over the instance defaults. */
function Limits({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const [own, setOwn] = useState<Caps | null>(null);
  const [draft, setDraft] = useState<Caps | null>(null);
  const [defaults, setDefaults] = useState<Caps>({});
  const [error, setError] = useState<string | null>(null);
  const save = useAction(setServerLimits);

  useEffect(() => {
    Promise.all([run(serverUsage(instanceKey, serverId)), run(nodeUsage(instanceKey))]).then(
      ([usage, node]) => {
        setOwn(caps(usage.ownLimits));
        setDraft(caps(usage.ownLimits));
        setDefaults(caps(node.defaultLimits));
      },
      (e) => setError(e.message),
    );
  }, [instanceKey, serverId]);

  if (error) return <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>;
  if (!own || !draft) return <div className="shimmer h-48 rounded-2xl" />;
  const changed = CAP_FIELDS.filter((f) => draft[f] !== own[f]).length;
  const fallback = (field: (typeof CAP_FIELDS)[number], bytes = false) => {
    const d = defaults[field];
    return `Instance default (${d === undefined ? "no limit" : bytes ? formatBytes(Number(d)) : Number(d).toLocaleString()})`;
  };
  const set = (field: (typeof CAP_FIELDS)[number]) => (value: bigint | undefined) => {
    setDraft((d) => ({ ...d, [field]: value }));
    save.setError(null);
  };

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-4">
        <p className="text-sm text-muted-foreground">A cap that's off follows the instance default, which you can change in the instance settings.</p>
        <Cap label="Members" value={draft.members} onChange={set("members")} placeholder={fallback("members")} />
        <Cap label="Channels" value={draft.channels} onChange={set("channels")} placeholder={fallback("channels")} />
        <Cap label="Storage" bytes value={draft.storageBytes} onChange={set("storageBytes")} placeholder={fallback("storageBytes", true)} />
        <Cap label="Files" bytes value={draft.attachmentBytes} onChange={set("attachmentBytes")} placeholder={fallback("attachmentBytes", true)} />
      </div>
      <SaveBar
        count={changed}
        saving={save.pending}
        error={save.error}
        nudge={0}
        onDiscard={() => setDraft(own)}
        onSave={async () => {
          const next = await save.go(instanceKey, serverId, draft);
          if (next) {
            setOwn(draft);
          }
        }}
      />
    </div>
  );
}

function Danger({ instanceKey, server, onDeleted }: { instanceKey: string; server: Server; onDeleted: () => void }) {
  const navigate = useNavigate();
  const [confirm, setConfirm] = useState("");
  const remove = useAction(deleteServer);
  const armed = confirm === server.name;
  async function submit(e: FormEvent) {
    e.preventDefault();
    const ok = await remove.go(instanceKey, server.id);
    if (ok === undefined) return;
    onDeleted();
    navigate({ to: "/$instance", params: { instance: instanceKey } });
  }
  return (
    <form onSubmit={submit} className="flex flex-col gap-3 rounded-2xl border border-destructive/40 bg-destructive/5 p-4">
      <p className="flex items-center gap-2 font-bold text-destructive">
        <TriangleAlertIcon className="size-4" /> Delete {server.name}
      </p>
      <p className="text-sm text-muted-foreground">
        Everyone loses access to its channels and messages. The server's operator keeps a copy of the file for a while, but you can't bring it back from here.
      </p>
      <Label htmlFor="confirm-delete" className="text-sm">
        Type <b>{server.name}</b> to confirm
      </Label>
      <Input
        id="confirm-delete"
        value={confirm}
        onChange={(e) => setConfirm(e.target.value)}
        className={cn("h-10 rounded-xl transition-colors", armed && "border-destructive ring-2 ring-destructive/20")}
        autoComplete="off"
      />
      {remove.error && <p className="text-sm text-destructive first-letter:uppercase">{remove.error}</p>}
      <motion.div
        className="self-end"
        initial={false}
        animate={armed ? { scale: [1, 1.08, 1], rotate: [0, -2, 2, 0] } : { scale: 1, rotate: 0 }}
        transition={{ duration: 0.4 }}
      >
        <Button type="submit" variant="destructive" disabled={!armed || remove.pending} className="rounded-xl font-bold">
          {remove.pending && <LoaderCircleIcon className="animate-spin" />}
          Delete server
        </Button>
      </motion.div>
    </form>
  );
}
