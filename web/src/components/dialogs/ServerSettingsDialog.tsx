import { useNavigate } from "@tanstack/react-router";
import {
  AtSignIcon,
  BellIcon,
  ChartColumnIcon,
  BadgeCheckIcon,
  BotIcon,
  BuildingIcon,
  CheckIcon,
  ChevronDownIcon,
  ClipboardListIcon,
  ClipboardPenIcon,
  CompassIcon,
  CrownIcon,
  DoorOpenIcon,
  EyeOffIcon,
  GaugeIcon,
  GavelIcon,
  HashIcon,
  HourglassIcon,
  InboxIcon,
  LinkIcon,
  LoaderCircleIcon,
  LockIcon,
  PartyPopperIcon,
  ScrollTextIcon,
  SettingsIcon,
  ShieldIcon,
  SmilePlusIcon,
  Trash2Icon,
  TriangleAlertIcon,
  UsersIcon,
  WebhookIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import type { GetServerUsageResponse } from "@/gen/fuwa/v1/server_pb";
import { AccountKind, ChannelType, NotificationLevel, Permission, type Server, type ServerLimits } from "@/gen/fuwa/v1/types_pb";
import { deleteServer, nodeUsage, run, serverUsage, setServerLimits, updateServer } from "@/fuwa/actions";
import { useAccess, useAction, useInstance } from "@/fuwa/hooks";
import { ServerIcon, UserAvatar } from "@/components/Icons";
import { PictureField } from "@/components/PictureField";
import { joinLine } from "@/components/chat/MessageList";
import { Applications } from "@/components/settings/server/Applications";
import { AuditLog } from "@/components/settings/server/AuditLog";
import { AutoMod } from "@/components/settings/server/AutoMod";
import { Emoji } from "@/components/settings/server/Emoji";
import { Webhooks } from "@/components/settings/server/Webhooks";
import { SingleSignOn } from "@/components/settings/server/SingleSignOn";
import { ServerAgents } from "@/components/settings/server/ServerAgents";
import { WelcomeScreenEditor } from "@/components/settings/server/WelcomeScreenEditor";
import { JoinFormEditor } from "@/components/settings/server/JoinFormEditor";
import { ServerDoor } from "@/components/join/ServerDoor";
import { useFuwa } from "@/fuwa/store";
import { Bans } from "@/components/settings/server/Bans";
import { Invites } from "@/components/settings/server/Invites";
import { Chips } from "@/components/settings/account/common";
import { Channels } from "@/components/settings/server/Channels";
import { Members } from "@/components/settings/server/Members";
import { Ownership } from "@/components/settings/server/Ownership";
import { Roles } from "@/components/settings/server/Roles";
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
import { Textarea } from "@/components/ui/textarea";
import { displayName, formatBytes, formatDuration, initials } from "@/lib/format";
import { ACCOUNT_AGES, timeLeft } from "@/lib/invites";
import { has } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { Choice, Cap, SaveBar, Toggle, WithPreview } from "@/components/settings/controls";
import { SettingsScreen } from "@/components/settings/SettingsScreen";

import { useServerSettingsTabs } from "@/components/dialogs/serverSettingsTabs";

export { useServerSettingsTabs };

export function ServerSettingsDialog({
  open,
  onOpenChange,
  instanceKey,
  server,
  tab: initialTab = "overview",
  target = null,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  server: Server;
  tab?: string;
  /** What to open the section on, such as a channel. */
  target?: string | null;
}) {
  const allowed = useServerSettingsTabs(instanceKey, server.id);
  const can = (id: string) => allowed.includes(id);
  const access = useAccess(instanceKey, server.id);
  const waiting = useFuwa((s) => s.instances[instanceKey]?.applications[server.id]?.length ?? 0);
  const [tab, setTab] = useState(initialTab);
  useEffect(() => {
    if (open) setTab(initialTab);
  }, [open, initialTab]);
  // Permissions can change while it's open: fall back to a section still yours.
  useEffect(() => {
    if (open && allowed.length && !allowed.includes(tab)) setTab(allowed[0]!);
  }, [open, allowed, tab]);
  const sections = [
    ...[{
      id: "overview",
      label: "Overview",
      icon: SettingsIcon,
      description: "How the server looks and how it greets people.",
      settings: [
        { id: "name", label: "Server name" },
        { id: "icon", label: "Server icon", keywords: "picture image upload logo avatar" },
        { id: "description", label: "Description" },
        { id: "join-messages", label: "Join messages", keywords: "system channel welcome greet" },
        { id: "default-notifications", label: "Default notifications", keywords: "mentions ping" },
      ],
    }],
    {
      id: "access",
      label: "Access",
      icon: DoorOpenIcon,
      description: "Who can join, and whether they apply first.",
      keywords: "join public private lock",
      settings: [
        { id: "discoverable", label: "Show in Browse", keywords: "discoverable public hidden invite only" },
        { id: "applications", label: "Apply to join", keywords: "applications review approve screening vetting questions" },
        { id: "linked-only", label: "waifu.dev accounts only", keywords: "linked verified account sign in" },
        { id: "account-age", label: "Minimum account age", keywords: "new accounts spam raid verification" },
      ],
    },
    {
      id: "sso",
      label: "Single sign-on",
      icon: BuildingIcon,
      description: "Members sign in through your organization's identity provider to join and to stay.",
      keywords: "sso saml oidc openid okta entra azure google workspace keycloak authentik identity provider organization company",
      settings: [
        { id: "sso-protocol", label: "Identity provider", keywords: "saml oidc openid" },
        { id: "sso-required", label: "Require single sign-on", keywords: "sso members join" },
        { id: "sso-recheck", label: "Sign in again", keywords: "sso recheck expire days" },
        { id: "sso-domains", label: "Email domains", keywords: "sso allowed" },
      ],
    },
    {
      id: "join-form",
      label: "Rules & questions",
      icon: ClipboardListIcon,
      description: "Rules new members agree to before they talk, and what people answer when they apply.",
      keywords: "rules screening agree questions application form onboarding",
      settings: [
        { id: "rules", label: "Rules", keywords: "screening agree code of conduct" },
        { id: "questions", label: "Application questions", keywords: "apply form" },
      ],
    },
    {
      id: "welcome",
      label: "Welcome screen",
      icon: PartyPopperIcon,
      description: "What new members see first: a few words and channels to start in.",
      keywords: "welcome onboarding new members greet suggested channels",
      settings: [
        { id: "welcome-enabled", label: "Show a welcome screen" },
        { id: "welcome-description", label: "Welcome message", keywords: "description" },
        { id: "welcome-channels", label: "Suggested channels", keywords: "start here" },
      ],
    },
    {
      id: "invites",
      label: "Invites",
      icon: LinkIcon,
      description: "Invite links that still work, and who made them.",
      keywords: "invite link code revoke expire uses",
    },
    {
      id: "roles",
      label: "Roles",
      icon: ShieldIcon,
      description: "Who can do what, ranked: colors, permissions and members for each role.",
      keywords: "permissions admin moderator rank color hoist mention everyone",
      settings: [
        { id: "role-permissions", label: "Role permissions", keywords: "administrator manage" },
        { id: "role-members", label: "Role members", keywords: "assign give" },
      ],
    },
    {
      id: "channels",
      label: "Channels",
      icon: HashIcon,
      description: "Order, categories, topics, slow mode, and who can see and use each.",
      keywords: "reorder drag category topic slowmode slow mode private permissions overwrites",
      settings: [
        { id: "slowmode", label: "Slow mode", keywords: "slowmode rate limit" },
        { id: "channel-permissions", label: "Channel permissions", keywords: "private hidden access roles" },
      ],
    },
    {
      id: "emoji",
      label: "Emoji",
      icon: SmilePlusIcon,
      description: "The server's own emoji. Everyone here can use them as :name:.",
      keywords: "emoji emote custom sticker upload",
    },
    {
      id: "integrations",
      label: "Integrations",
      icon: WebhookIcon,
      description: "Agents, accounts programs drive, and webhooks, addresses other apps post messages to.",
      keywords: "webhook webhooks integration apps bot bots agent agents ci github feed rss alerts post api discord",
      settings: [
        { id: "agents", label: "Agents", keywords: "bot add username" },
        { id: "webhooks", label: "Webhooks", keywords: "address url token" },
      ],
    },
    { id: "usage", label: "Usage", icon: ChartColumnIcon, description: "What the server holds, against its caps.", keywords: "storage members messages" },
    {
      id: "limits",
      label: "Limits",
      icon: GaugeIcon,
      description: "Caps for this server only, over the instance's defaults.",
      keywords: "caps members channels storage",
    },
  ].filter((s) => can(s.id));
  const peopleSections = [
    {
      id: "applications",
      label: "Applications",
      icon: InboxIcon,
      badge: waiting,
      description: "People asking to join, with their answers.",
      keywords: "apply review approve reject let in turn down pending waiting",
    },
    { id: "members", label: "Members", icon: UsersIcon, description: "Roles, nicknames, time-outs, kicks and bans.", keywords: "admin role kick ban timeout nickname" },
    { id: "bans", label: "Bans", icon: GavelIcon, description: "Who's kept out, and why.", keywords: "unban banned" },
    {
      id: "automod",
      label: "AutoMod",
      icon: BotIcon,
      description: "Rules that catch messages as they're sent: blocked words, mention spam, links and a smart filter.",
      keywords: "automod auto moderation filter blocked words banned words swear profanity spam mentions pings raid links urls block alert time out ai smart jev clef typesafe cloudflare hate scam",
    },
    { id: "audit-log", label: "Audit log", icon: ScrollTextIcon, description: "Every change people made here.", keywords: "history log moderation" },
  ].filter((s) => can(s.id));
  const people = peopleSections.length ? [{ label: "People", sections: peopleSections }] : [];
  const danger = [
    { id: "ownership", label: "Transfer ownership", icon: CrownIcon, danger: true, keywords: "owner hand give" },
    { id: "danger", label: "Delete server", icon: Trash2Icon, danger: true, keywords: "remove" },
  ].filter((s) => can(s.id));
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
      {tab === "overview" && can("overview") && <Overview instanceKey={instanceKey} server={server} />}
      {tab === "access" && can("access") && <Access instanceKey={instanceKey} server={server} />}
      {tab === "sso" && can("sso") && <SingleSignOn instanceKey={instanceKey} server={server} />}
      {tab === "join-form" && can("join-form") && <JoinFormEditor instanceKey={instanceKey} server={server} onOpenAccess={() => setTab("access")} />}
      {tab === "welcome" && can("welcome") && <WelcomeScreenEditor instanceKey={instanceKey} server={server} />}
      {tab === "emoji" && can("emoji") && <Emoji instanceKey={instanceKey} serverId={server.id} />}
      {tab === "integrations" && can("integrations") && (
        <div className="flex flex-col gap-8">
          {has(access, Permission.MANAGE_SERVER) && <ServerAgents instanceKey={instanceKey} serverId={server.id} />}
          {has(access, Permission.MANAGE_WEBHOOKS) && <Webhooks instanceKey={instanceKey} serverId={server.id} />}
        </div>
      )}
      {tab === "automod" && can("automod") && <AutoMod instanceKey={instanceKey} serverId={server.id} />}
      {tab === "invites" && can("invites") && <Invites instanceKey={instanceKey} serverId={server.id} />}
      {tab === "roles" && can("roles") && <Roles instanceKey={instanceKey} serverId={server.id} initial={target} />}
      {tab === "channels" && can("channels") && <Channels instanceKey={instanceKey} serverId={server.id} initial={target} />}
      {tab === "usage" && can("usage") && <Usage instanceKey={instanceKey} serverId={server.id} />}
      {tab === "limits" && can("limits") && <Limits instanceKey={instanceKey} serverId={server.id} />}
      {tab === "applications" && can("applications") && (
        <Applications instanceKey={instanceKey} serverId={server.id} takesApplications={server.applications} />
      )}
      {tab === "members" && can("members") && <Members instanceKey={instanceKey} serverId={server.id} />}
      {tab === "bans" && can("bans") && <Bans instanceKey={instanceKey} serverId={server.id} />}
      {tab === "audit-log" && can("audit-log") && <AuditLog instanceKey={instanceKey} serverId={server.id} />}
      {tab === "ownership" && can("ownership") && <Ownership instanceKey={instanceKey} server={server} onDone={() => setTab(allowed[0] ?? "overview")} />}
      {tab === "danger" && can("danger") && <Danger instanceKey={instanceKey} server={server} onDeleted={() => onOpenChange(false)} />}
    </SettingsScreen>
  );
}

const onlyMentions = (level: NotificationLevel) => level === NotificationLevel.MENTIONS;

function Overview({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const inst = useInstance(instanceKey);
  const textChannels = (inst?.channels[server.id] ?? []).filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT);
  const [name, setName] = useState(server.name);
  const [iconUrl, setIconUrl] = useState(server.iconUrl);
  const [description, setDescription] = useState(server.description);
  const [systemChannel, setSystemChannel] = useState(server.systemChannelId);
  const [mentionsOnly, setMentionsOnly] = useState(onlyMentions(server.defaultNotifications));
  const save = useAction(updateServer);
  const changes = [
    name !== server.name,
    iconUrl !== server.iconUrl,
    description !== server.description,
    systemChannel !== server.systemChannelId,
    mentionsOnly !== onlyMentions(server.defaultNotifications),
  ].filter(Boolean).length;

  function discard() {
    setName(server.name);
    setIconUrl(server.iconUrl);
    setDescription(server.description);
    setSystemChannel(server.systemChannelId);
    setMentionsOnly(onlyMentions(server.defaultNotifications));
    save.setError(null);
  }

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    if (!name.trim()) return save.setError("a server needs a name");
    if (iconUrl.trim() && !/^https?:\/\/\S+$/i.test(iconUrl.trim())) return save.setError("icon links start with https://");
    await save.go(instanceKey, server.id, {
      ...(name !== server.name && { name: name.trim() }),
      ...(iconUrl !== server.iconUrl && { iconUrl: iconUrl.trim() }),
      ...(description !== server.description && { description: description.trim() }),
      ...(systemChannel !== server.systemChannelId && { systemChannelId: systemChannel }),
      ...(mentionsOnly !== onlyMentions(server.defaultNotifications) && {
        defaultNotifications: mentionsOnly ? NotificationLevel.MENTIONS : NotificationLevel.UNSPECIFIED,
      }),
    });
  }

  const shown = { ...server, name: name || server.name, iconUrl: /^https?:\/\//i.test(iconUrl.trim()) ? iconUrl.trim() : "", description };
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
          <div data-setting="name" className="flex flex-col gap-2 border-b border-border/70 pb-5">
            <Label htmlFor="settings-name" className="font-extrabold">
              Name
            </Label>
            <Input id="settings-name" required maxLength={100} value={name} onChange={(e) => setName(e.target.value)} className="h-11 rounded-xl" />
          </div>
          <div data-setting="icon" className="flex flex-col gap-2 border-b border-border/70 py-5">
            <span className="font-extrabold">Icon</span>
            <span className="text-sm text-muted-foreground">Square pictures work best. GIFs keep moving. Without one it's the server's initials.</span>
            <PictureField
              instanceKey={instanceKey}
              kind="icon"
              value={iconUrl}
              onChange={setIconUrl}
              fallback={
                <motion.span key={initials(shown.name)} initial={{ scale: 0.85, rotate: -8 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 600, damping: 16 }} className="block size-full">
                  <ServerIcon server={{ ...shown, iconUrl: "" }} active className="size-full text-2xl" />
                </motion.span>
              }
            />
          </div>
          <div data-setting="description" className="flex flex-col gap-2 border-b border-border/70 py-5">
            <Label htmlFor="settings-description" className="font-extrabold">
              Description
            </Label>
            <Textarea id="settings-description" rows={4} maxLength={1000} value={description} onChange={(e) => setDescription(e.target.value)} className="rounded-xl" />
            <p className="text-sm text-muted-foreground">Shown in Browse. Markdown works.</p>
          </div>
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

/**
 * Who can join: anyone who finds it in Browse, or only people with an invite;
 * whether they join straight away or apply first; waifu.dev accounts only;
 * and how old their account must be.
 */
function Access({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const inst = useInstance(instanceKey);
  const linkedOffered = !!inst?.node?.auth?.linkedSignIn;
  const [discoverable, setDiscoverable] = useState(server.discoverable);
  const [applications, setApplications] = useState(server.applications);
  const [linkedOnly, setLinkedOnly] = useState(server.linkedOnly);
  const [minAge, setMinAge] = useState(server.minAccountAgeSeconds);
  const save = useAction(updateServer);
  const changes = [
    discoverable !== server.discoverable,
    applications !== server.applications,
    linkedOnly !== server.linkedOnly,
    minAge !== server.minAccountAgeSeconds,
  ].filter(Boolean).length;
  // A value set some other way (the API, an older client) still shows as a choice.
  const ages = ACCOUNT_AGES.some((a) => a.value === server.minAccountAgeSeconds)
    ? ACCOUNT_AGES
    : [...ACCOUNT_AGES, { value: server.minAccountAgeSeconds, label: formatDuration(server.minAccountAgeSeconds) }].sort((a, b) => a.value - b.value);
  const shown = { ...server, discoverable, applications, linkedOnly, minAccountAgeSeconds: minAge };

  function discard() {
    setDiscoverable(server.discoverable);
    setApplications(server.applications);
    setLinkedOnly(server.linkedOnly);
    setMinAge(server.minAccountAgeSeconds);
    save.setError(null);
  }

  async function submit() {
    await save.go(instanceKey, server.id, {
      ...(discoverable !== server.discoverable && { discoverable }),
      ...(applications !== server.applications && { applications }),
      ...(linkedOnly !== server.linkedOnly && { linkedOnly }),
      ...(minAge !== server.minAccountAgeSeconds && { minAccountAgeSeconds: minAge }),
    });
  }

  return (
    <WithPreview
      preview={
        <div className="flex flex-col gap-4">
          <BrowseCard server={shown} />
          <GatePreview minAge={minAge} applications={applications} linkedOnly={linkedOnly} />
        </div>
      }
    >
      <div className="flex flex-col">
        <div data-setting="discoverable" className="flex flex-col gap-3 border-b border-border/70 pb-5">
          <span>
            <span className="block font-extrabold">Who can join</span>
            <span className="block text-sm text-muted-foreground">Invite links work either way, for anyone allowed to make them.</span>
          </span>
          <Choice
            value={discoverable ? "browse" : "invite"}
            onChange={(v) => setDiscoverable(v === "browse")}
            options={[
              { value: "invite", label: "Invite only", hint: "Hidden from Browse. People join with an invite link.", icon: <LockIcon className="size-4" /> },
              { value: "browse", label: "Anyone here", hint: "Listed in Browse for everyone on this fuwa server.", icon: <CompassIcon className="size-4" /> },
            ]}
          />
        </div>
        <div data-setting="applications" className="flex flex-col gap-3 border-b border-border/70 py-5">
          <span>
            <span className="block font-extrabold">How people get in</span>
            <span className="block text-sm text-muted-foreground">
              Applications wait under Applications for anyone who can kick members. Set the questions under Rules &amp; questions.
            </span>
          </span>
          <Choice
            value={applications ? "apply" : "join"}
            onChange={(v) => setApplications(v === "apply")}
            options={[
              { value: "join", label: "Join straight away", hint: "Anyone who finds it or has an invite is in at once.", icon: <DoorOpenIcon className="size-4" /> },
              {
                value: "apply",
                label: "Apply to join",
                hint: "People answer your questions and wait for someone to let them in.",
                icon: <ClipboardPenIcon className="size-4" />,
              },
            ]}
          />
          <AnimatePresence initial={false}>
            {server.applications && !applications && (
              <motion.p initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="overflow-hidden text-xs text-amber-600 dark:text-amber-400">
                Applications still waiting are dropped. Those people can join straight away instead.
              </motion.p>
            )}
          </AnimatePresence>
        </div>
        <div data-setting="linked-only" className="flex flex-col gap-2 border-b border-border/70 py-5">
          <Toggle
            checked={linkedOnly}
            onChange={setLinkedOnly}
            disabled={!linkedOffered && !linkedOnly}
            label="waifu.dev accounts only"
            hint={
              linkedOffered || linkedOnly
                ? "Only people who sign in with waifu.dev can join or apply. Accounts made on this fuwa server can't. Members already here stay."
                : "This fuwa server doesn't offer waifu.dev sign-in, so nobody could join."
            }
          />
        </div>
        <div data-setting="account-age" className="flex flex-col gap-3 py-5">
          <span>
            <span className="block font-extrabold">Minimum account age</span>
            <span className="block text-sm text-muted-foreground">
              Accounts newer than this wait before they can join or apply, by invite or from Browse. It keeps throwaway accounts out during a raid.
            </span>
          </span>
          <Chips label="Minimum account age" value={minAge} options={ages} onChange={setMinAge} />
        </div>
      </div>
      <SaveBar count={changes} saving={save.pending} error={save.error} onSave={() => void submit()} onDiscard={discard} />
    </WithPreview>
  );
}

/** Three accounts at the door, and what happens to each: in, applying, waiting out the age limit, or kept out for not being waifu.dev. */
function GatePreview({ minAge, applications, linkedOnly }: { minAge: number; applications: boolean; linkedOnly: boolean }) {
  const people = [
    { name: "2 hours old", sub: "waifu.dev account", age: 2 * 3600, kind: AccountKind.LINKED, hue: 330 },
    { name: "A month old", sub: "waifu.dev account", age: 30 * 86_400, kind: AccountKind.LINKED, hue: 200 },
    { name: "A month old", sub: "Made on this server", age: 30 * 86_400, kind: AccountKind.LOCAL, hue: 140 },
  ];
  return (
    <div className="flex flex-col gap-2.5 rounded-3xl border bg-card p-4 shadow-lg">
      <p className="text-xs font-bold text-muted-foreground">At the door</p>
      {people.map((p, n) => {
        const wait = minAge - p.age;
        const out = linkedOnly && p.kind !== AccountKind.LINKED;
        const state = out ? "out" : wait > 0 ? `wait-${wait}` : applications ? "apply" : "in";
        return (
          <div key={n} className="flex items-center gap-2.5 text-sm">
            <span className="size-7 shrink-0 rounded-full" style={{ background: `linear-gradient(135deg, oklch(0.75 0.14 ${p.hue}), oklch(0.6 0.16 ${p.hue + 40}))` }} />
            <span className="min-w-0 flex-1">
              <span className="block truncate leading-tight">{p.name}</span>
              <span className="flex items-center gap-1 truncate text-[0.7rem] text-muted-foreground">
                {p.kind === AccountKind.LINKED && <BadgeCheckIcon className="size-3" />}
                {p.sub}
              </span>
            </span>
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.span
                key={state}
                initial={{ scale: 0.6, opacity: 0, rotate: state === "in" ? -20 : 20 }}
                animate={{ scale: 1, opacity: 1, rotate: 0 }}
                exit={{ scale: 0.6, opacity: 0 }}
                transition={{ type: "spring", stiffness: 600, damping: 18 }}
                className={cn(
                  "flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-xs font-bold",
                  state === "in" && "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400",
                  state === "apply" && "bg-primary/15 text-primary",
                  state === "out" && "bg-destructive/15 text-destructive",
                  state.startsWith("wait") && "bg-amber-500/15 text-amber-600 dark:text-amber-400",
                )}
              >
                {state === "in" ? (
                  <CheckIcon className="size-3" strokeWidth={3} />
                ) : state === "apply" ? (
                  <ClipboardPenIcon className="size-3" />
                ) : state === "out" ? (
                  <XIcon className="size-3" strokeWidth={3} />
                ) : (
                  <HourglassIcon className="size-3" />
                )}
                {state === "in" ? "Joins" : state === "apply" ? "Applies" : state === "out" ? "Can't join" : `Waits ${timeLeft(wait * 1000)}`}
              </motion.span>
            </AnimatePresence>
          </div>
        );
      })}
    </div>
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
        <ServerDoor server={server} />
        <span className="btn grid h-9 place-items-center rounded-xl bg-primary text-sm font-bold text-primary-foreground">
          <SwapText>{server.applications ? "Apply to join" : "Join"}</SwapText>
        </span>
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
              <span className="text-xs text-muted-foreground">People join with an invite link.</span>
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
    { label: "Emoji", value: Number(u.emojis), limit: l?.emojis },
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
const CAP_FIELDS = ["members", "channels", "storageBytes", "attachmentBytes", "emojis"] as const;
const caps = (l: ServerLimits | undefined): Caps => ({
  members: l?.members,
  channels: l?.channels,
  storageBytes: l?.storageBytes,
  attachmentBytes: l?.attachmentBytes,
  emojis: l?.emojis,
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
        <Cap label="Emoji" value={draft.emojis} onChange={set("emojis")} placeholder={fallback("emojis")} />
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
