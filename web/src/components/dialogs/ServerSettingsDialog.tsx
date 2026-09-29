import { useNavigate } from "@tanstack/react-router";
import {
  AtSignIcon,
  BellIcon,
  ChartColumnIcon,
  ChevronDownIcon,
  CrownIcon,
  EyeOffIcon,
  GaugeIcon,
  GavelIcon,
  HashIcon,
  LoaderCircleIcon,
  ScrollTextIcon,
  SettingsIcon,
  Trash2Icon,
  TriangleAlertIcon,
  UsersIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import type { GetServerUsageResponse } from "@/gen/fuwa/v1/server_pb";
import { ChannelType, MemberRole, NotificationLevel, type Server, type ServerLimits } from "@/gen/fuwa/v1/types_pb";
import { deleteServer, nodeUsage, run, serverUsage, setServerLimits, updateServer } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { ServerIcon, UserAvatar } from "@/components/Icons";
import { joinLine } from "@/components/chat/MessageList";
import { AuditLog } from "@/components/settings/server/AuditLog";
import { Bans } from "@/components/settings/server/Bans";
import { Channels } from "@/components/settings/server/Channels";
import { Members } from "@/components/settings/server/Members";
import { Ownership } from "@/components/settings/server/Ownership";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { InlineMarkdown } from "@/components/Markdown";
import { Count, CountUp, SPRING, SwapText } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { displayName, formatBytes, initials } from "@/lib/format";
import { cn } from "@/lib/utils";
import { Choice, Cap, SaveBar, WithPreview } from "@/components/settings/controls";
import { SettingsScreen } from "@/components/settings/SettingsScreen";

export function ServerSettingsDialog({
  open,
  onOpenChange,
  instanceKey,
  server,
  role,
  instanceAdmin = false,
  tab: initialTab = "overview",
  target = null,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  server: Server;
  /** Your role in the server. Owners and admins manage it; only owners hand it on. */
  role: MemberRole;
  /** Instance admins also get the server's own caps, and can delete it. */
  instanceAdmin?: boolean;
  tab?: string;
  /** What to open the section on, such as a channel. */
  target?: string | null;
}) {
  const [tab, setTab] = useState(initialTab);
  useEffect(() => {
    if (open) setTab(initialTab);
  }, [open, initialTab]);
  const manager = role >= MemberRole.ADMIN;
  const owner = role === MemberRole.OWNER;
  const sections = [
    {
      id: "overview",
      label: "Overview",
      icon: SettingsIcon,
      description: "How the server looks, whether people can find it, and how it greets them.",
      settings: [
        { id: "name", label: "Server name" },
        { id: "description", label: "Description" },
        { id: "discoverable", label: "Show in Browse", keywords: "discoverable public hidden" },
        { id: "join-messages", label: "Join messages", keywords: "system channel welcome greet" },
        { id: "default-notifications", label: "Default notifications", keywords: "mentions ping" },
      ],
    },
    ...(manager
      ? [
          {
            id: "channels",
            label: "Channels",
            icon: HashIcon,
            description: "Order, categories, topics and slow mode.",
            keywords: "reorder drag category topic slowmode slow mode",
            settings: [{ id: "slowmode", label: "Slow mode", keywords: "slowmode rate limit" }],
          },
        ]
      : []),
    { id: "usage", label: "Usage", icon: ChartColumnIcon, description: "What the server holds, against its caps.", keywords: "storage members messages" },
    ...(instanceAdmin
      ? [
          {
            id: "limits",
            label: "Limits",
            icon: GaugeIcon,
            description: "Caps for this server only, over the instance's defaults.",
            keywords: "caps members channels storage",
          },
        ]
      : []),
  ];
  const people = manager
    ? [
        {
          label: "People",
          sections: [
            { id: "members", label: "Members", icon: UsersIcon, description: "Roles, nicknames, time-outs, kicks and bans.", keywords: "admin role kick ban timeout nickname" },
            { id: "bans", label: "Bans", icon: GavelIcon, description: "Who's kept out, and why.", keywords: "unban banned" },
            { id: "audit-log", label: "Audit log", icon: ScrollTextIcon, description: "What owners and admins did here.", keywords: "history log moderation" },
          ],
        },
      ]
    : [];
  const danger = [
    ...(owner ? [{ id: "ownership", label: "Transfer ownership", icon: CrownIcon, danger: true, keywords: "owner hand give" }] : []),
    ...(owner || instanceAdmin ? [{ id: "danger", label: "Delete server", icon: Trash2Icon, danger: true, keywords: "remove" }] : []),
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
      groups={[{ label: server.name, sections }, ...people, ...(danger.length ? [{ sections: danger }] : [])]}
    >
      {tab === "overview" && <Overview instanceKey={instanceKey} server={server} />}
      {tab === "channels" && manager && <Channels instanceKey={instanceKey} serverId={server.id} initial={target} />}
      {tab === "usage" && <Usage instanceKey={instanceKey} serverId={server.id} />}
      {tab === "limits" && instanceAdmin && <Limits instanceKey={instanceKey} serverId={server.id} />}
      {tab === "members" && manager && <Members instanceKey={instanceKey} serverId={server.id} />}
      {tab === "bans" && manager && <Bans instanceKey={instanceKey} serverId={server.id} />}
      {tab === "audit-log" && manager && <AuditLog instanceKey={instanceKey} serverId={server.id} />}
      {tab === "ownership" && owner && <Ownership instanceKey={instanceKey} server={server} onDone={() => setTab("overview")} />}
      {tab === "danger" && (owner || instanceAdmin) && <Danger instanceKey={instanceKey} server={server} onDeleted={() => onOpenChange(false)} />}
    </SettingsScreen>
  );
}

const onlyMentions = (level: NotificationLevel) => level === NotificationLevel.MENTIONS;

function Overview({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const inst = useInstance(instanceKey);
  const textChannels = (inst?.channels[server.id] ?? []).filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT);
  const [name, setName] = useState(server.name);
  const [description, setDescription] = useState(server.description);
  const [discoverable, setDiscoverable] = useState(server.discoverable);
  const [systemChannel, setSystemChannel] = useState(server.systemChannelId);
  const [mentionsOnly, setMentionsOnly] = useState(onlyMentions(server.defaultNotifications));
  const save = useAction(updateServer);
  const changes = [
    name !== server.name,
    description !== server.description,
    discoverable !== server.discoverable,
    systemChannel !== server.systemChannelId,
    mentionsOnly !== onlyMentions(server.defaultNotifications),
  ].filter(Boolean).length;

  function discard() {
    setName(server.name);
    setDescription(server.description);
    setDiscoverable(server.discoverable);
    setSystemChannel(server.systemChannelId);
    setMentionsOnly(onlyMentions(server.defaultNotifications));
    save.setError(null);
  }

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    if (!name.trim()) return save.setError("a server needs a name");
    await save.go(instanceKey, server.id, {
      ...(name !== server.name && { name: name.trim() }),
      ...(description !== server.description && { description: description.trim() }),
      ...(discoverable !== server.discoverable && { discoverable }),
      ...(systemChannel !== server.systemChannelId && { systemChannelId: systemChannel }),
      ...(mentionsOnly !== onlyMentions(server.defaultNotifications) && {
        defaultNotifications: mentionsOnly ? NotificationLevel.MENTIONS : NotificationLevel.UNSPECIFIED,
      }),
    });
  }

  const shown = { ...server, name: name || server.name, description, discoverable };
  const greeting = textChannels.find((c) => c.id === systemChannel);
  return (
    <form onSubmit={submit}>
      <WithPreview
        preview={
          <div className="flex flex-col gap-4">
            <BrowseCard server={shown} />
            <JoinPreview channelName={greeting?.name ?? null} user={inst?.me ?? undefined} />
          </div>
        }
      >
        <div className="flex flex-col">
          <div data-setting="name" className="flex items-center gap-4 border-b border-border/70 pb-5">
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
          <div data-setting="description" className="flex flex-col gap-2 border-b border-border/70 py-5">
            <Label htmlFor="settings-description" className="font-extrabold">
              Description
            </Label>
            <Textarea id="settings-description" rows={4} maxLength={1000} value={description} onChange={(e) => setDescription(e.target.value)} className="rounded-xl" />
            <p className="text-sm text-muted-foreground">Shown in Browse. Markdown works.</p>
          </div>
          <label data-setting="discoverable" className="flex cursor-pointer items-center justify-between gap-4 border-b border-border/70 py-5">
            <span>
              <span className="block font-extrabold">Show in Browse</span>
              <span className="block text-sm text-muted-foreground">Anyone on this fuwa server can find and join it.</span>
            </span>
            <Switch checked={discoverable} onCheckedChange={setDiscoverable} />
          </label>
          <div data-setting="join-messages" className="flex flex-col gap-2 border-b border-border/70 py-5">
            <span className="font-extrabold">Join messages</span>
            <span className="text-sm text-muted-foreground">A hello in a channel whenever someone joins, so people can wave.</span>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="group flex h-11 items-center gap-2 rounded-xl border px-3 text-left text-sm transition hover:border-primary/40 data-[state=open]:border-primary/60"
                >
                  <HashIcon className="size-4 text-muted-foreground" />
                  <span className="flex-1 truncate font-bold">{greeting?.name ?? "Don't post them"}</span>
                  <ChevronDownIcon className="size-4 text-muted-foreground transition-transform duration-300 group-data-[state=open]:rotate-180" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" className="max-h-72 w-64 overflow-y-auto">
                <DropdownMenuRadioGroup value={systemChannel} onValueChange={setSystemChannel}>
                  <DropdownMenuRadioItem value="">Don't post them</DropdownMenuRadioItem>
                  {textChannels.map((c) => (
                    <DropdownMenuRadioItem key={c.id} value={c.id}>
                      #{c.name}
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
          <div data-setting="default-notifications" className="flex flex-col gap-3 py-5">
            <span>
              <span className="block font-extrabold">Default notifications</span>
              <span className="block text-sm text-muted-foreground">What members hear about until they pick for themselves.</span>
            </span>
            <Choice
              value={mentionsOnly ? "mentions" : "own"}
              onChange={(v) => setMentionsOnly(v === "mentions")}
              options={[
                { value: "own", label: "Their own setting", hint: "Each device decides, as it does everywhere else.", icon: <BellIcon className="size-4" /> },
                { value: "mentions", label: "Only @mentions", hint: "Quieter, for busy servers.", icon: <AtSignIcon className="size-4" /> },
              ]}
            />
          </div>
        </div>
        <SaveBar count={changes} saving={save.pending} error={save.error} onSave={() => void submit()} onDiscard={discard} />
      </WithPreview>
    </form>
  );
}

/** A join message as it will look, or a note that there won't be one. */
function JoinPreview({ channelName, user }: { channelName: string | null; user: Parameters<typeof UserAvatar>[0]["user"] }) {
  return (
    <div className="overflow-hidden rounded-3xl border bg-card p-4 shadow-lg">
      <p className="mb-2 flex items-center gap-1 text-xs font-bold text-muted-foreground">
        <HashIcon className="size-3.5" />
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span key={channelName ?? "none"} initial={{ y: 10, opacity: 0 }} animate={{ y: 0, opacity: 1 }} exit={{ y: -10, opacity: 0 }} transition={SPRING}>
            {channelName ?? "no channel"}
          </motion.span>
        </AnimatePresence>
      </p>
      <motion.div animate={{ opacity: channelName ? 1 : 0.35, filter: channelName ? "blur(0px)" : "blur(2px)" }} className="flex items-center gap-2 text-sm">
        <motion.span animate={channelName ? { x: [0, 4, 0] } : { x: 0 }} transition={{ duration: 1.6, repeat: Infinity }} className="text-emerald-500">
          →
        </motion.span>
        <UserAvatar user={user} className="size-6" />
        <span className="min-w-0 truncate">{joinLine(user?.id ?? "", displayName(user))}</span>
      </motion.div>
    </div>
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
