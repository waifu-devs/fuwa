import { useNavigate } from "@tanstack/react-router";
import { LoaderCircleIcon, TriangleAlertIcon } from "lucide-react";
import { motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import type { GetServerUsageResponse } from "@/gen/fuwa/v1/server_pb";
import type { Server, ServerLimits } from "@/gen/fuwa/v1/types_pb";
import { deleteServer, nodeUsage, run, serverUsage, setServerLimits, updateServer } from "@/fuwa/actions";
import { useAction } from "@/fuwa/hooks";
import { SlidingNumber } from "@/components/animate-ui/primitives/texts/sliding-number";
import { ServerIcon } from "@/components/Icons";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsContents, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { formatBytes } from "@/lib/format";
import { Cap, SaveBar } from "@/components/settings/controls";

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
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent wide>
        <DialogHeader title={`${server.name} settings`} />
        <Tabs value={tab} onValueChange={setTab}>
          <TabsList className="w-full">
            <TabsTrigger value="overview">Overview</TabsTrigger>
            <TabsTrigger value="usage">Usage</TabsTrigger>
            {instanceAdmin && <TabsTrigger value="limits">Limits</TabsTrigger>}
            {isOwner && <TabsTrigger value="danger">Danger zone</TabsTrigger>}
          </TabsList>
          <TabsContents className="pt-4">
            <TabsContent value="overview">
              <Overview instanceKey={instanceKey} server={server} onSaved={() => onOpenChange(false)} />
            </TabsContent>
            <TabsContent value="usage">{open && tab === "usage" && <Usage instanceKey={instanceKey} serverId={server.id} />}</TabsContent>
            {instanceAdmin && (
              <TabsContent value="limits">{open && tab === "limits" && <Limits instanceKey={instanceKey} serverId={server.id} />}</TabsContent>
            )}
            {isOwner && (
              <TabsContent value="danger">
                <Danger instanceKey={instanceKey} server={server} onDeleted={() => onOpenChange(false)} />
              </TabsContent>
            )}
          </TabsContents>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}

function Overview({ instanceKey, server, onSaved }: { instanceKey: string; server: Server; onSaved: () => void }) {
  const [name, setName] = useState(server.name);
  const [description, setDescription] = useState(server.description);
  const [discoverable, setDiscoverable] = useState(server.discoverable);
  const save = useAction(updateServer);
  const changed = name !== server.name || description !== server.description || discoverable !== server.discoverable;

  async function submit(e: FormEvent) {
    e.preventDefault();
    const ok = await save.go(instanceKey, server.id, {
      ...(name !== server.name && { name: name.trim() }),
      ...(description !== server.description && { description: description.trim() }),
      ...(discoverable !== server.discoverable && { discoverable }),
    });
    if (ok !== undefined) onSaved();
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <div className="flex items-center gap-4">
        <ServerIcon server={{ ...server, name: name || server.name }} active className="size-16 text-xl" />
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <Label htmlFor="settings-name" className="font-bold">
            Name
          </Label>
          <Input id="settings-name" required maxLength={100} value={name} onChange={(e) => setName(e.target.value)} className="h-11 rounded-xl" />
        </div>
      </div>
      <div className="flex flex-col gap-2">
        <Label htmlFor="settings-description" className="font-bold">
          Description
        </Label>
        <Textarea id="settings-description" rows={3} maxLength={1000} value={description} onChange={(e) => setDescription(e.target.value)} className="rounded-xl" />
      </div>
      <label className="flex cursor-pointer items-center justify-between gap-4 rounded-2xl border p-3">
        <span>
          <span className="block text-sm font-bold">Show in Browse</span>
          <span className="block text-xs text-muted-foreground">Anyone on this fuwa server can find and join it.</span>
        </span>
        <Switch checked={discoverable} onCheckedChange={setDiscoverable} />
      </label>
      {save.error && <p className="text-sm text-destructive first-letter:uppercase">{save.error}</p>}
      <Button type="submit" disabled={!changed || save.pending || !name.trim()} className="btn h-11 self-end rounded-xl px-6 font-bold">
        {save.pending && <LoaderCircleIcon className="animate-spin" />}
        Save changes
      </Button>
    </form>
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
                {r.bytes ? formatBytes(r.value) : <SlidingNumber number={r.value} />}
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
      <div className="flex flex-col gap-3 rounded-2xl border bg-background/40 p-4">
        <p className="text-xs text-muted-foreground">
          Caps for this server only. A cap that's off follows the instance default, which you can change in the instance settings.
        </p>
        <Cap label="Members" value={draft.members} onChange={set("members")} placeholder={fallback("members")} />
        <Cap label="Channels" value={draft.channels} onChange={set("channels")} placeholder={fallback("channels")} />
        <Cap label="Storage" bytes value={draft.storageBytes} onChange={set("storageBytes")} placeholder={fallback("storageBytes", true)} />
        <Cap label="Files" bytes value={draft.attachmentBytes} onChange={set("attachmentBytes")} placeholder={fallback("attachmentBytes", true)} />
      </div>
      <SaveBar
        inset
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
  async function submit(e: FormEvent) {
    e.preventDefault();
    const ok = await remove.go(instanceKey, server.id);
    if (ok === undefined && remove.error) return;
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
      <Input id="confirm-delete" value={confirm} onChange={(e) => setConfirm(e.target.value)} className="h-10 rounded-xl" autoComplete="off" />
      {remove.error && <p className="text-sm text-destructive first-letter:uppercase">{remove.error}</p>}
      <Button type="submit" variant="destructive" disabled={confirm !== server.name || remove.pending} className="self-end rounded-xl font-bold">
        {remove.pending && <LoaderCircleIcon className="animate-spin" />}
        Delete server
      </Button>
    </form>
  );
}
