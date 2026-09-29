import { useNavigate } from "@tanstack/react-router";
import { LoaderCircleIcon, TriangleAlertIcon } from "lucide-react";
import { motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import type { GetServerUsageResponse } from "@/gen/fuwa/v1/server_pb";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { deleteServer, run, serverUsage, updateServer } from "@/fuwa/actions";
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

export function ServerSettingsDialog({
  open,
  onOpenChange,
  instanceKey,
  server,
  isOwner,
  tab: initialTab = "overview",
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  server: Server;
  isOwner: boolean;
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
            {isOwner && <TabsTrigger value="danger">Danger zone</TabsTrigger>}
          </TabsList>
          <TabsContents className="pt-4">
            <TabsContent value="overview">
              <Overview instanceKey={instanceKey} server={server} onSaved={() => onOpenChange(false)} />
            </TabsContent>
            <TabsContent value="usage">{open && tab === "usage" && <Usage instanceKey={instanceKey} serverId={server.id} />}</TabsContent>
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
