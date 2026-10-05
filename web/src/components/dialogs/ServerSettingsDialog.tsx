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
  MapPinIcon,
  PartyPopperIcon,
  ScrollTextIcon,
  SettingsIcon,
  ShieldIcon,
  SmilePlusIcon,
  Trash2Icon,
  TriangleAlertIcon,
  UsersIcon,
  VideoIcon,
  WebhookIcon,
  XIcon,
  type LucideIcon,
} from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import type { GetServerUsageResponse } from "@/gen/fuwa/v1/server_pb";
import { AccountKind, ChannelType, NotificationLevel, Permission, type Server, type ServerLimits } from "@/gen/fuwa/v1/types_pb";
import { deleteServer, nodeUsage, run, serverUsage, setServerLimits, updateServer } from "@/fuwa/actions";
import { useAccess, useAction, useInstance } from "@/fuwa/hooks";
import { ServerIcon, UserAvatar } from "@/components/Icons";
import { PictureField } from "@/components/PictureField";
import { joinLine } from "@/components/chat/join-line";
import { Applications } from "@/components/settings/server/Applications";
import { AuditLog } from "@/components/settings/server/AuditLog";
import { AutoMod } from "@/components/settings/server/AutoMod";
import { Emoji } from "@/components/settings/server/Emoji";
import { Webhooks } from "@/components/settings/server/Webhooks";
import { SharedChannels } from "@/components/settings/server/SharedChannels";
import { SharedGlyph } from "@/components/chat/Shared";
import { SharedConnectionState } from "@/gen/fuwa/v1/channel_pb";
import { listConnections } from "@/fuwa/actions";
import { SingleSignOn } from "@/components/settings/server/SingleSignOn";
import { RecordingSettings } from "@/components/settings/server/Recordings";
import { ServerAgents } from "@/components/settings/server/ServerAgents";
import { WelcomeAndOnboarding } from "@/components/settings/server/WelcomeAndOnboarding";
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
import { Count, CountUp, SwapText } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { displayName, formatBytes, formatDuration, initials } from "@/lib/format";
import { T, useI18n } from "@/i18n/react";
import { ACCOUNT_AGES, timeLeft } from "@/lib/invites";
import { has } from "@/lib/permissions";
import { hasRegions, regionName } from "@/lib/regions";
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
  const { t } = useI18n();
  const allowed = useServerSettingsTabs(instanceKey, server.id);
  const can = (id: string) => allowed.includes(id);
  const access = useAccess(instanceKey, server.id);
  const managesShared = allowed.includes("shared");
  useEffect(() => {
    if (open && managesShared) run(listConnections(instanceKey, server.id)).catch(() => {});
  }, [open, managesShared, instanceKey, server.id]);
  const [picked, setTab] = useState(initialTab);
  // Opening it (or asking for another section while open) starts on the section asked for.
  const opening = open ? initialTab : null;
  const [lastOpening, setLastOpening] = useState(opening);
  if (lastOpening !== opening) {
    setLastOpening(opening);
    if (opening !== null) setTab(opening);
  }
  // Permissions can change while it's open: fall back to a section still yours.
  const tab = open && allowed.length && !allowed.includes(picked) ? allowed[0]! : picked;
  const groups = useSettingsGroups(instanceKey, server, can);
  return (
    <SettingsScreen
      open={open}
      onOpenChange={onOpenChange}
      title={server.name}
      subtitle={t("serversettings.nav.subtitle")}
      section={tab}
      onSectionChange={setTab}
      openToSection={initialTab !== "overview"}
      groups={groups}
    >
      {can(tab) && PAGES[tab]?.({ instanceKey, server, access, target, allowed, setTab, close: () => onOpenChange(false) })}
    </SettingsScreen>
  );
}

/** The menu: this server's sections, the people ones, and the dangerous ones, each only if yours. */
function useSettingsGroups(instanceKey: string, server: Server, can: (id: string) => boolean) {
  const { t } = useI18n();
  const waiting = useFuwa((s) => s.instances[instanceKey]?.applications[server.id]?.length ?? 0);
  // Other servers asking to show one of this server's channels, for the menu's badge.
  const sharedRequests = useFuwa(
    (s) => s.instances[instanceKey]?.shared[server.id]?.connections.filter((c) => c.home && c.state === SharedConnectionState.WAITING).length ?? 0,
  );
  const sections = [
    ...[{
      id: "overview",
      label: t("serversettings.nav.overview"),
      icon: SettingsIcon,
      description: t("serversettings.nav.overviewAbout"),
      settings: [
        { id: "name", label: t("serversettings.nav.serverName") },
        { id: "icon", label: t("serversettings.nav.serverIcon"), keywords: "picture image upload logo avatar" },
        { id: "description", label: t("serversettings.nav.description") },
        { id: "join-messages", label: t("serversettings.nav.joinMessages"), keywords: "system channel welcome greet" },
        { id: "default-notifications", label: t("serversettings.nav.defaultNotifications"), keywords: "mentions ping" },
      ],
    }],
    {
      id: "access",
      label: t("serversettings.nav.access"),
      icon: DoorOpenIcon,
      description: t("serversettings.nav.accessAbout"),
      keywords: "join public private lock",
      settings: [
        { id: "discoverable", label: t("serversettings.nav.discoverable"), keywords: "discoverable public hidden invite only" },
        { id: "applications", label: t("serversettings.nav.applyToJoin"), keywords: "applications review approve screening vetting questions" },
        { id: "linked-only", label: t("serversettings.nav.linkedOnly"), keywords: "linked verified account sign in" },
        { id: "account-age", label: t("serversettings.nav.accountAge"), keywords: "new accounts spam raid verification" },
      ],
    },
    {
      id: "sso",
      label: t("serversettings.nav.sso"),
      icon: BuildingIcon,
      description: t("serversettings.nav.ssoAbout"),
      keywords: "sso saml oidc openid okta entra azure google workspace keycloak authentik identity provider organization company",
      settings: [
        { id: "sso-protocol", label: t("serversettings.nav.ssoProtocol"), keywords: "saml oidc openid" },
        { id: "sso-required", label: t("serversettings.nav.ssoRequired"), keywords: "sso members join" },
        { id: "sso-recheck", label: t("serversettings.nav.ssoRecheck"), keywords: "sso recheck expire days" },
        { id: "sso-domains", label: t("serversettings.nav.ssoDomains"), keywords: "sso allowed" },
      ],
    },
    {
      id: "join-form",
      label: t("serversettings.nav.joinForm"),
      icon: ClipboardListIcon,
      description: t("serversettings.nav.joinFormAbout"),
      keywords: "rules screening agree questions application form onboarding",
      settings: [
        { id: "rules", label: t("serversettings.nav.rules"), keywords: "screening agree code of conduct" },
        { id: "questions", label: t("serversettings.nav.questions"), keywords: "apply form" },
      ],
    },
    {
      id: "welcome",
      label: t("serversettings.nav.welcome"),
      icon: PartyPopperIcon,
      description: t("serversettings.nav.welcomeAbout"),
      keywords: "welcome onboarding new members greet suggested channels banner header cover accent color interests",
      settings: [
        { id: "banner-picture", label: t("settings.nav.banner"), keywords: "header cover picture image" },
        { id: "banner-focus", label: t("serversettings.nav.bannerFocus"), keywords: "crop position" },
        { id: "accent-color", label: t("serversettings.nav.accentColor"), keywords: "colour tint theme" },
        { id: "welcome-enabled", label: t("serversettings.nav.welcomeEnabled") },
        { id: "welcome-description", label: t("serversettings.nav.welcomeMessage"), keywords: "description" },
        { id: "welcome-channels", label: t("serversettings.nav.suggestedChannels"), keywords: "start here" },
        { id: "onboarding-enabled", label: t("serversettings.nav.onboarding"), keywords: "steps interests" },
        { id: "onboarding-steps", label: t("serversettings.nav.onboardingSteps"), keywords: "pick interests roles channels rules hello" },
      ],
    },
    {
      id: "invites",
      label: t("serversettings.nav.invites"),
      icon: LinkIcon,
      description: t("serversettings.nav.invitesAbout"),
      keywords: "invite link code revoke expire uses",
    },
    {
      id: "roles",
      label: t("serversettings.nav.roles"),
      icon: ShieldIcon,
      description: t("serversettings.nav.rolesAbout"),
      keywords: "permissions admin moderator rank color hoist mention everyone",
      settings: [
        { id: "role-permissions", label: t("serversettings.nav.rolePermissions"), keywords: "administrator manage" },
        { id: "role-members", label: t("serversettings.nav.roleMembers"), keywords: "assign give" },
      ],
    },
    {
      id: "channels",
      label: t("serversettings.nav.channels"),
      icon: HashIcon,
      description: t("serversettings.nav.channelsAbout"),
      keywords: "reorder drag category topic slowmode slow mode private permissions overwrites",
      settings: [
        { id: "slowmode", label: t("serversettings.nav.slowmode"), keywords: "slowmode rate limit" },
        { id: "channel-permissions", label: t("serversettings.nav.channelPermissions"), keywords: "private hidden access roles" },
      ],
    },
    {
      id: "emoji",
      label: t("serversettings.nav.emoji"),
      icon: SmilePlusIcon,
      description: t("serversettings.nav.emojiAbout"),
      keywords: "emoji emote custom sticker upload",
    },
    {
      id: "integrations",
      label: t("serversettings.nav.integrations"),
      icon: WebhookIcon,
      description: t("serversettings.nav.integrationsAbout"),
      keywords: "webhook webhooks integration apps bot bots agent agents ci github feed rss alerts post api discord",
      settings: [
        { id: "agents", label: t("settings.nav.agents"), keywords: "bot add username" },
        { id: "webhooks", label: t("serversettings.nav.webhooks"), keywords: "address url token" },
      ],
    },
    {
      id: "shared",
      label: t("serversettings.nav.shared"),
      icon: SharedGlyph as unknown as LucideIcon,
      badge: sharedRequests,
      description: t("serversettings.nav.sharedAbout"),
      keywords: "share connect slack connect other server guest home code external partner",
    },
    {
      id: "recordings",
      label: t("serversettings.nav.recordings"),
      icon: VideoIcon,
      description: t("serversettings.nav.recordingsAbout"),
      keywords: "record recording call voice video camera screen webm",
      settings: [{ id: "record-video", label: t("serversettings.nav.recordVideo"), keywords: "camera screen share webm" }],
    },
    { id: "usage", label: t("serversettings.nav.usage"), icon: ChartColumnIcon, description: t("serversettings.nav.usageAbout"), keywords: "storage members messages" },
    {
      id: "limits",
      label: t("serversettings.nav.limits"),
      icon: GaugeIcon,
      description: t("serversettings.nav.limitsAbout"),
      keywords: "caps members channels storage",
    },
  ].filter((s) => can(s.id));
  const peopleSections = [
    {
      id: "applications",
      label: t("serversettings.nav.applications"),
      icon: InboxIcon,
      badge: waiting,
      description: t("serversettings.nav.applicationsAbout"),
      keywords: "apply review approve reject let in turn down pending waiting",
    },
    {
      id: "members",
      label: t("serversettings.nav.members"),
      icon: UsersIcon,
      description: t("serversettings.nav.membersAbout"),
      keywords: "admin role kick ban timeout nickname",
    },
    { id: "bans", label: t("serversettings.nav.bans"), icon: GavelIcon, description: t("serversettings.nav.bansAbout"), keywords: "unban banned" },
    {
      id: "automod",
      label: t("serversettings.nav.automod"),
      icon: BotIcon,
      description: t("serversettings.nav.automodAbout"),
      keywords: "automod auto moderation filter blocked words banned words swear profanity spam mentions pings raid links urls block alert time out ai smart jev clef typesafe cloudflare hate scam",
    },
    { id: "audit-log", label: t("serversettings.nav.auditLog"), icon: ScrollTextIcon, description: t("serversettings.nav.auditLogAbout"), keywords: "history log moderation" },
  ].filter((s) => can(s.id));
  const people = peopleSections.length ? [{ label: t("serversettings.nav.people"), sections: peopleSections }] : [];
  const danger = [
    { id: "ownership", label: t("serversettings.nav.ownership"), icon: CrownIcon, danger: true, keywords: "owner hand give" },
    { id: "danger", label: t("serversettings.nav.danger"), icon: Trash2Icon, danger: true, keywords: "remove" },
  ].filter((s) => can(s.id));
  return [{ label: server.name, sections }, ...people, ...(danger.length ? [{ sections: danger }] : [])];
}

type PageProps = {
  instanceKey: string;
  server: Server;
  access: ReturnType<typeof useAccess>;
  target: string | null;
  allowed: string[];
  setTab: (tab: string) => void;
  close: () => void;
};

/** What each section shows. */
const PAGES: Record<string, (p: PageProps) => ReactNode> = {
  overview: (p) => <Overview instanceKey={p.instanceKey} server={p.server} />,
  access: (p) => <Access instanceKey={p.instanceKey} server={p.server} />,
  sso: (p) => <SingleSignOn instanceKey={p.instanceKey} server={p.server} />,
  "join-form": (p) => <JoinFormEditor instanceKey={p.instanceKey} server={p.server} onOpenAccess={() => p.setTab("access")} />,
  welcome: (p) => <WelcomeAndOnboarding instanceKey={p.instanceKey} server={p.server} />,
  emoji: (p) => <Emoji instanceKey={p.instanceKey} serverId={p.server.id} />,
  integrations: (p) => (
    <div className="flex flex-col gap-8">
      {has(p.access, Permission.MANAGE_SERVER) && <ServerAgents instanceKey={p.instanceKey} serverId={p.server.id} />}
      {has(p.access, Permission.MANAGE_WEBHOOKS) && <Webhooks instanceKey={p.instanceKey} serverId={p.server.id} />}
    </div>
  ),
  shared: (p) => <SharedChannels instanceKey={p.instanceKey} serverId={p.server.id} />,
  automod: (p) => <AutoMod instanceKey={p.instanceKey} serverId={p.server.id} />,
  invites: (p) => <Invites instanceKey={p.instanceKey} serverId={p.server.id} />,
  roles: (p) => <Roles instanceKey={p.instanceKey} serverId={p.server.id} initial={p.target} />,
  channels: (p) => <Channels instanceKey={p.instanceKey} serverId={p.server.id} initial={p.target} />,
  recordings: (p) => <RecordingSettings instanceKey={p.instanceKey} server={p.server} />,
  usage: (p) => <Usage instanceKey={p.instanceKey} serverId={p.server.id} />,
  limits: (p) => <Limits instanceKey={p.instanceKey} serverId={p.server.id} />,
  applications: (p) => <Applications instanceKey={p.instanceKey} serverId={p.server.id} takesApplications={p.server.applications} />,
  members: (p) => <Members instanceKey={p.instanceKey} serverId={p.server.id} />,
  bans: (p) => <Bans instanceKey={p.instanceKey} serverId={p.server.id} />,
  "audit-log": (p) => <AuditLog instanceKey={p.instanceKey} serverId={p.server.id} />,
  ownership: (p) => <Ownership instanceKey={p.instanceKey} server={p.server} onDone={() => p.setTab(p.allowed[0] ?? "overview")} />,
  danger: (p) => <Danger instanceKey={p.instanceKey} server={p.server} onDeleted={p.close} />,
};

const onlyMentions = (level: NotificationLevel) => level === NotificationLevel.MENTIONS;

function Overview({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const textChannels = (inst?.channels[server.id] ?? []).filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT);
  const [name, setName] = useState(server.name);
  const [iconUrl, setIconUrl] = useState(server.iconUrl);
  const [description, setDescription] = useState(server.description);
  const [systemChannel, setSystemChannel] = useState(server.systemChannelId);
  const [mentionsOnly, setMentionsOnly] = useState(() => onlyMentions(server.defaultNotifications));
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
    if (!name.trim()) return save.setError(t("serversettings.overview.needsName"));
    if (iconUrl.trim() && !/^https?:\/\/\S+$/i.test(iconUrl.trim())) return save.setError(t("serversettings.overview.iconLink"));
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
              {t("serversettings.overview.name")}
            </Label>
            <Input id="settings-name" required maxLength={100} value={name} onChange={(e) => setName(e.target.value)} className="h-11 rounded-xl" />
          </div>
          <div data-setting="icon" className="flex flex-col gap-2 border-b border-border/70 py-5">
            <span className="font-extrabold">{t("serversettings.overview.icon")}</span>
            <span className="text-sm text-muted-foreground">{t("serversettings.overview.iconHint")}</span>
            <PictureField
              instanceKey={instanceKey}
              kind="icon"
              serverId={server.id}
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
              {t("serversettings.nav.description")}
            </Label>
            <Textarea id="settings-description" rows={4} maxLength={1000} value={description} onChange={(e) => setDescription(e.target.value)} className="rounded-xl" />
            <p className="text-sm text-muted-foreground">{t("serversettings.overview.descriptionHint")}</p>
          </div>
          {hasRegions(inst?.node?.regions) && (
            <div data-setting="region" className="flex items-center gap-3 border-b border-border/70 py-5">
              <motion.span
                initial={{ scale: 0.6, rotate: -20 }}
                animate={{ scale: 1, rotate: 0 }}
                transition={{ type: "spring", stiffness: 500, damping: 18 }}
                className="grid size-10 shrink-0 place-items-center rounded-xl bg-primary/15 text-primary"
              >
                <MapPinIcon className="size-5" />
              </motion.span>
              <span className="min-w-0">
                <span className="block font-extrabold">{t("serversettings.overview.region", { region: regionName(inst?.node?.regions, server.region) })}</span>
                <span className="block text-sm text-muted-foreground">{t("serversettings.overview.regionHint")}</span>
              </span>
            </div>
          )}
          <div data-setting="join-messages" className="flex flex-col gap-2 border-b border-border/70 py-5">
            <span className="font-extrabold">{t("serversettings.nav.joinMessages")}</span>
            <span className="text-sm text-muted-foreground">{t("serversettings.overview.joinMessagesHint")}</span>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="group flex h-11 items-center gap-2 rounded-xl border px-3 text-left text-sm transition hover:border-primary/40 data-[state=open]:border-primary/60"
                >
                  <HashIcon className="size-4 text-muted-foreground" />
                  <span className="flex-1 truncate font-bold">{greeting?.name ?? t("serversettings.overview.dontPost")}</span>
                  <ChevronDownIcon className="size-4 text-muted-foreground transition-transform duration-300 group-data-[state=open]:rotate-180" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" className="max-h-72 w-64 overflow-y-auto">
                <DropdownMenuRadioGroup value={systemChannel} onValueChange={setSystemChannel}>
                  <DropdownMenuRadioItem value="">{t("serversettings.overview.dontPost")}</DropdownMenuRadioItem>
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
              <span className="block font-extrabold">{t("serversettings.nav.defaultNotifications")}</span>
              <span className="block text-sm text-muted-foreground">{t("serversettings.overview.defaultNotificationsHint")}</span>
            </span>
            <Choice
              value={mentionsOnly ? "mentions" : "own"}
              onChange={(v) => setMentionsOnly(v === "mentions")}
              options={[
                { value: "own", label: t("serversettings.overview.ownSetting"), hint: t("serversettings.overview.ownSettingHint"), icon: <BellIcon className="size-4" /> },
                { value: "mentions", label: t("common.notify.mentions"), hint: t("serversettings.overview.mentionsHint"), icon: <AtSignIcon className="size-4" /> },
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
  const lang = useI18n();
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
  const known = ACCOUNT_AGES.map((a) => ({ value: a.value, label: lang.t(a.label) }));
  const ages = ACCOUNT_AGES.some((a) => a.value === server.minAccountAgeSeconds)
    ? known
    : [...known, { value: server.minAccountAgeSeconds, label: formatDuration(lang, server.minAccountAgeSeconds) }].sort((a, b) => a.value - b.value);
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
            <span className="block font-extrabold">{lang.t("serversettings.access.whoCanJoin")}</span>
            <span className="block text-sm text-muted-foreground">{lang.t("serversettings.access.whoCanJoinHint")}</span>
          </span>
          <Choice
            value={discoverable ? "browse" : "invite"}
            onChange={(v) => setDiscoverable(v === "browse")}
            options={[
              { value: "invite", label: lang.t("serversettings.access.inviteOnly"), hint: lang.t("serversettings.access.inviteOnlyHint"), icon: <LockIcon className="size-4" /> },
              { value: "browse", label: lang.t("serversettings.access.anyone"), hint: lang.t("serversettings.access.anyoneHint"), icon: <CompassIcon className="size-4" /> },
            ]}
          />
        </div>
        <div data-setting="applications" className="flex flex-col gap-3 border-b border-border/70 py-5">
          <span>
            <span className="block font-extrabold">{lang.t("serversettings.access.howIn")}</span>
            <span className="block text-sm text-muted-foreground">{lang.t("serversettings.access.howInHint")}</span>
          </span>
          <Choice
            value={applications ? "apply" : "join"}
            onChange={(v) => setApplications(v === "apply")}
            options={[
              { value: "join", label: lang.t("serversettings.access.joinNow"), hint: lang.t("serversettings.access.joinNowHint"), icon: <DoorOpenIcon className="size-4" /> },
              {
                value: "apply",
                label: lang.t("serversettings.nav.applyToJoin"),
                hint: lang.t("serversettings.access.applyHint"),
                icon: <ClipboardPenIcon className="size-4" />,
              },
            ]}
          />
          <AnimatePresence initial={false}>
            {server.applications && !applications && (
              <motion.p initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="overflow-hidden text-xs text-amber-600 dark:text-amber-400">
                {lang.t("serversettings.access.dropped")}
              </motion.p>
            )}
          </AnimatePresence>
        </div>
        <div data-setting="linked-only" className="flex flex-col gap-2 border-b border-border/70 py-5">
          <Toggle
            checked={linkedOnly}
            onChange={setLinkedOnly}
            disabled={!linkedOffered && !linkedOnly}
            label={lang.t("serversettings.nav.linkedOnly")}
            hint={linkedOffered || linkedOnly ? lang.t("serversettings.access.linkedOnlyHint") : lang.t("serversettings.access.linkedUnavailable")}
          />
        </div>
        <div data-setting="account-age" className="flex flex-col gap-3 py-5">
          <span>
            <span className="block font-extrabold">{lang.t("serversettings.nav.accountAge")}</span>
            <span className="block text-sm text-muted-foreground">{lang.t("serversettings.access.accountAgeHint")}</span>
          </span>
          <Chips label={lang.t("serversettings.nav.accountAge")} value={minAge} options={ages} onChange={setMinAge} />
        </div>
      </div>
      <SaveBar count={changes} saving={save.pending} error={save.error} onSave={() => void submit()} onDiscard={discard} />
    </WithPreview>
  );
}

/** Three accounts at the door, and what happens to each: in, applying, waiting out the age limit, or kept out for not being waifu.dev. */
function GatePreview({ minAge, applications, linkedOnly }: { minAge: number; applications: boolean; linkedOnly: boolean }) {
  const lang = useI18n();
  const { t } = lang;
  const people = [
    { name: t("serversettings.access.gate.hoursOld"), sub: t("serversettings.access.gate.linked"), age: 2 * 3600, kind: AccountKind.LINKED, hue: 330 },
    { name: t("serversettings.access.gate.monthOld"), sub: t("serversettings.access.gate.linked"), age: 30 * 86_400, kind: AccountKind.LINKED, hue: 200 },
    { name: t("serversettings.access.gate.monthOld"), sub: t("serversettings.access.gate.local"), age: 30 * 86_400, kind: AccountKind.LOCAL, hue: 140 },
  ];
  return (
    <div className="flex flex-col gap-2.5 rounded-3xl border bg-card p-4 shadow-lg">
      <p className="text-xs font-bold text-muted-foreground">{t("serversettings.access.gate.title")}</p>
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
                {state === "in"
                  ? t("serversettings.access.gate.joins")
                  : state === "apply"
                    ? t("serversettings.access.gate.applies")
                    : state === "out"
                      ? t("serversettings.access.gate.cantJoin")
                      : t("serversettings.access.gate.waits", { time: timeLeft(lang, wait * 1000) })}
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
  const { t } = useI18n();
  return (
    <div className="overflow-hidden rounded-3xl border bg-card p-4 shadow-lg">
      <p className="mb-2 flex items-center gap-1 text-xs font-bold text-muted-foreground">
        <HashIcon className="size-3.5" />
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span key={channelName ?? "none"} initial={{ y: 10, opacity: 0 }} animate={{ y: 0, opacity: 1 }} exit={{ y: -10, opacity: 0 }} transition={SPRING}>
            {channelName ?? t("serversettings.overview.noChannel")}
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
  const { t } = useI18n();
  const members = Number(server.memberCount);
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
              <UsersIcon className="size-3.5" /> <T k="serversettings.shared.members" values={{ count: <Count value={members} /> }} count={members} />
            </p>
          </div>
        </div>
        <p className="line-clamp-4 text-sm break-words text-muted-foreground">
          {server.description.trim() ? <InlineMarkdown>{server.description}</InlineMarkdown> : t("serversettings.browse.noDescription")}
        </p>
        <ServerDoor server={server} />
        <span className="btn grid h-9 place-items-center rounded-xl bg-primary text-sm font-bold text-primary-foreground">
          <SwapText>{server.applications ? t("serversettings.nav.applyToJoin") : t("serversettings.browse.join")}</SwapText>
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
              <span className="text-sm font-extrabold">{t("serversettings.browse.hidden")}</span>
              <span className="text-xs text-muted-foreground">{t("serversettings.browse.hiddenHint")}</span>
            </span>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function Usage({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const lang = useI18n();
  const [data, setData] = useState<GetServerUsageResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    run(serverUsage(instanceKey, serverId)).then(setData, (e) => setError(e.message));
  }, [instanceKey, serverId]);

  if (error) return <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>;
  if (!data?.usage) return <div className="grid gap-3 sm:grid-cols-2">{[0, 1, 2, 3].map((n) => <div key={n} className="shimmer h-24 rounded-2xl" />)}</div>;
  const u = data.usage;
  const l = data.limits;
  const checks = Number(u.automodChecksToday);
  const checkCap = l?.automodChecksPerDay === undefined ? null : Number(l.automodChecksPerDay);
  const { t } = lang;
  const rows: { label: string; value: number; limit?: bigint; bytes?: boolean; sub?: string; note?: string; warn?: boolean }[] = [
    { label: t("serversettings.nav.members"), value: Number(u.members), limit: l?.members },
    { label: t("serversettings.nav.channels"), value: Number(u.channels), limit: l?.channels },
    { label: t("serversettings.usage.messages"), value: Number(u.messages), sub: t("serversettings.usage.sentAllTime", { count: Number(u.messagesSent) }) },
    { label: t("serversettings.usage.storage"), value: Number(u.storageBytes), limit: l?.storageBytes, bytes: true },
    {
      label: t("serversettings.usage.attachments"),
      value: Number(u.attachmentBytes),
      limit: l?.attachmentBytes,
      bytes: true,
      sub: t("serversettings.usage.files", { count: Number(u.attachments) }),
    },
    { label: t("serversettings.nav.emoji"), value: Number(u.emojis), limit: l?.emojis },
    { label: t("serversettings.usage.events"), value: Number(u.events), sub: t("serversettings.usage.inLog") },
    {
      label: t("serversettings.usage.smartChecks"),
      value: checks,
      limit: l?.automodChecksPerDay,
      sub: t("serversettings.usage.noDailyLimit"),
      warn: checkCap !== null && checks >= checkCap,
      note:
        checkCap === null
          ? undefined
          : checks >= checkCap
            ? t("serversettings.usage.allUsed", { count: checkCap })
            : t("serversettings.usage.checksLeft", { cap: lang.number(checkCap), left: lang.number(checkCap - checks) }),
    },
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
                <CountUp value={r.value} delay={0.1 + n * 0.05} format={r.bytes ? (v) => formatBytes(lang, Math.round(v)) : undefined} />
              </p>
              <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-muted">
                <motion.div
                  className={cn("h-full rounded-full", r.warn ? "bg-amber-500" : "bg-primary")}
                  initial={{ x: "-100%" }}
                  animate={{ x: limit ? `${share * 100 - 100}%` : "0%", opacity: limit ? 1 : 0.25 }}
                  transition={{ duration: 0.9, ease: [0.22, 1, 0.36, 1], delay: 0.1 + n * 0.05 }}
                />
              </div>
              <p className={cn("mt-1.5 text-xs", r.warn ? "font-bold text-amber-700 dark:text-amber-400" : "text-muted-foreground")}>
                {r.note ?? (limit !== null ? t("serversettings.usage.of", { limit: r.bytes ? formatBytes(lang, limit) : lang.number(limit) }) : (r.sub ?? t("serversettings.usage.noLimit")))}
              </p>
            </motion.div>
          );
        })}
      </div>
      <p className="text-xs text-muted-foreground">{t("serversettings.usage.note")}</p>
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
  const lang = useI18n();
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
    return lang.t("serversettings.limits.instanceDefault", {
      value: d === undefined ? lang.t("serversettings.usage.noLimit") : bytes ? formatBytes(lang, Number(d)) : lang.number(Number(d)),
    });
  };
  const set = (field: (typeof CAP_FIELDS)[number]) => (value: bigint | undefined) => {
    setDraft((d) => ({ ...d, [field]: value }));
    save.setError(null);
  };

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-4">
        <p className="text-sm text-muted-foreground">{lang.t("serversettings.limits.intro")}</p>
        <Cap label={lang.t("serversettings.nav.members")} value={draft.members} onChange={set("members")} placeholder={fallback("members")} />
        <Cap label={lang.t("serversettings.nav.channels")} value={draft.channels} onChange={set("channels")} placeholder={fallback("channels")} />
        <Cap label={lang.t("serversettings.usage.storage")} bytes value={draft.storageBytes} onChange={set("storageBytes")} placeholder={fallback("storageBytes", true)} />
        <Cap label={lang.t("serversettings.limits.files")} bytes value={draft.attachmentBytes} onChange={set("attachmentBytes")} placeholder={fallback("attachmentBytes", true)} />
        <Cap label={lang.t("serversettings.nav.emoji")} value={draft.emojis} onChange={set("emojis")} placeholder={fallback("emojis")} />
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
  const { t } = useI18n();
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
        <TriangleAlertIcon className="size-4" /> {t("serversettings.danger.title", { server: server.name })}
      </p>
      <p className="text-sm text-muted-foreground">{t("serversettings.danger.hint")}</p>
      <Label htmlFor="confirm-delete" className="text-sm">
        <T k="serversettings.danger.confirm" values={{ name: <b>{server.name}</b> }} />
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
          {t("serversettings.nav.danger")}
        </Button>
      </motion.div>
    </form>
  );
}
