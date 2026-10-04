import { clone, create } from "@bufbuild/protobuf";
import {
  CrownIcon,
  DoorClosedIcon,
  DoorOpenIcon,
  Flower2Icon,
  GlobeIcon,
  LinkIcon,
  LockIcon,
  ShieldCheckIcon,
  UsersIcon,
  BanIcon,
  GaugeIcon,
  MegaphoneIcon,
  ServerIcon,
  SlidersHorizontalIcon,
  UserPlusIcon,
  BotIcon,
  BuildingIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type ReactNode } from "react";
import {
  InstanceSettingsSchema,
  LinkedAccounts,
  LocalAccounts,
  SsoAccounts,
  type InstanceConfig,
  type InstanceSettings,
} from "@/gen/fuwa/v1/admin_pb";
import { AccountKind, AgentCreation, ServerCreation, ServerLimitsSchema } from "@/gen/fuwa/v1/types_pb";
import { getSettings, run, startSsoSignIn, updateSettings } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Private, usePrivateField } from "@/components/Private";
import { formatBytes } from "@/lib/format";
import { canReturnTo, WAIFU_DEV_ISSUER } from "@/lib/linked";
import { HIDDEN_ADDRESS } from "@/lib/streamer";
import { cn } from "@/lib/utils";
import { Cap, Choice, SaveBar, Setting, SPRING, Toggle } from "./controls";
import { fullProvider, IdentityProviderForm, providerFingerprint } from "./IdentityProviderForm";
import { providerReady } from "@/lib/sso";
import type { IdentityProvider } from "@/gen/fuwa/v1/sso_pb";
import { Accounts } from "./instance/Accounts";
import { Announcement } from "./instance/Announcement";
import { CALL_FIELDS, CALL_SECTION, CallSettings } from "./instance/Calls";
import { FEDERATION_FIELDS, FEDERATION_SECTION, FederationSettings } from "./instance/Federation";
import { MODERATION_FIELDS, MODERATION_SECTION, ModerationSettings } from "./instance/Moderation";
import { Servers } from "./instance/Servers";
import { SettingsScreen } from "./SettingsScreen";

/** Every setting, as the API names it, and how to read it for comparing. */
const FIELDS: { path: string; get: (s: InstanceSettings) => unknown }[] = [
  { path: "name", get: (s) => s.name.trim() },
  { path: "public_url", get: (s) => s.publicUrl.trim() },
  { path: "allowed_origins", get: (s) => s.allowedOrigins.map((o) => o.trim()).filter(Boolean).join("\n") },
  { path: "local_accounts", get: (s) => s.localAccounts },
  { path: "linked_accounts", get: (s) => s.linkedAccounts },
  { path: "linked_issuer", get: (s) => s.linkedIssuer.trim().replace(/\/+$/, "") },
  { path: "sso_accounts", get: (s) => s.ssoAccounts },
  { path: "sso_provider", get: (s) => providerFingerprint(s.ssoProvider) },
  { path: "server_creation", get: (s) => s.serverCreation },
  { path: "agent_creation", get: (s) => s.agentCreation },
  { path: "shared_channels", get: (s) => s.sharedChannels },
  { path: "profile_effects", get: (s) => s.profileEffects },
  { path: "servers_per_account", get: (s) => s.serversPerAccount },
  { path: "default_limits.members", get: (s) => s.defaultLimits?.members },
  { path: "default_limits.channels", get: (s) => s.defaultLimits?.channels },
  { path: "default_limits.storage_bytes", get: (s) => s.defaultLimits?.storageBytes },
  { path: "default_limits.attachment_bytes", get: (s) => s.defaultLimits?.attachmentBytes },
  { path: "default_limits.emojis", get: (s) => s.defaultLimits?.emojis },
  { path: "default_limits.recording_bytes", get: (s) => s.defaultLimits?.recordingBytes },
  { path: "picture_upload_bytes", get: (s) => s.pictureUploadBytes },
  { path: "picture_upload_bytes_per_day", get: (s) => s.pictureUploadBytesPerDay },
  { path: "poll_votes_per_minute", get: (s) => s.pollVotesPerMinute },
  { path: "telemetry", get: (s) => s.telemetry },
  { path: "web", get: (s) => s.web },
  ...CALL_FIELDS,
  ...MODERATION_FIELDS,
  ...FEDERATION_FIELDS,
];

const changedPaths = (draft: InstanceSettings, saved: InstanceSettings) =>
  FIELDS.filter((f) => f.get(draft) !== f.get(saved)).map((f) => f.path);

const count = (n: bigint | undefined) => (n === undefined ? "no limit" : Number(n).toLocaleString());
const size = (n: bigint | undefined) => (n === undefined ? "no limit" : formatBytes(Number(n)));

/**
 * Everything an admin can change about a fuwa instance while it runs. Each
 * setting starts from the operator's environment; changes are stored on the
 * instance, and Reset puts one back.
 */
export function InstanceSettingsDialog({
  open,
  onOpenChange,
  instanceKey,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
}) {
  const inst = useInstance(instanceKey);
  const [config, setConfig] = useState<InstanceConfig | null>(null);
  const [draft, setDraft] = useState<InstanceSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tab, setTab] = useState("general");
  const save = useAction(updateSettings);
  const test = useAction(startSsoSignIn);
  const privateField = usePrivateField();

  useEffect(() => {
    if (!open) return;
    setConfig(null);
    setLoadError(null);
    setTab("general");
    run(getSettings(instanceKey)).then(
      (c) => {
        setConfig(c);
        setDraft(clone(InstanceSettingsSchema, c.settings!));
      },
      (e) => setLoadError(e.message),
    );
  }, [open, instanceKey]);

  const saved = config?.settings;
  const changed = saved && draft ? changedPaths(draft, saved) : [];
  const overridden = new Set(config?.overridden ?? []);
  const defaults = config?.defaults;

  function patch(fn: (d: InstanceSettings) => void) {
    setDraft((d) => {
      if (!d) return d;
      const next = clone(InstanceSettingsSchema, d);
      next.defaultLimits ??= create(ServerLimitsSchema);
      fn(next);
      return next;
    });
    save.setError(null);
  }

  async function commit(update: string[], reset: string[]) {
    if (!draft) return;
    const next = await save.go(instanceKey, draft, update, reset);
    if (!next) return;
    setConfig(next);
    // Keep unsaved edits to other settings; take the stored value for the rest.
    setDraft((d) => {
      const fresh = clone(InstanceSettingsSchema, next.settings!);
      if (!d || !saved) return fresh;
      const pending = changedPaths(d, saved).filter((p) => !update.includes(p) && !reset.includes(p));
      return pending.length ? mergeFields(fresh, d, pending) : fresh;
    });
  }

  const resetter = (...paths: string[]) => ({
    changed: paths.some((p) => overridden.has(p)),
    onReset: () => commit([], paths),
    resetting: save.pending,
  });

  const name = inst?.node?.name ?? instanceKey;
  const loading = !config || !draft || !defaults;
  const hasPasswordHere = inst?.me?.kind === AccountKind.LOCAL;
  const patchProvider = (fn: (p: IdentityProvider) => void) =>
    patch((d) => {
      d.ssoProvider = fullProvider(d.ssoProvider);
      fn(d.ssoProvider);
    });

  return (
    <SettingsScreen
      open={open}
      onOpenChange={onOpenChange}
      title={name}
      subtitle="Instance settings"
      section={tab}
      onSectionChange={setTab}
      groups={[
        {
          label: "Instance",
          sections: [
            {
              id: "general",
              label: "General",
              icon: SlidersHorizontalIcon,
              description: `What everyone on ${name} gets. Changes apply right away.`,
              settings: [
                { id: "name", label: "Name" },
                { id: "public-url", label: "Public address", keywords: "url domain" },
                { id: "web", label: "Web app" },
                { id: "origins", label: "Sites that can connect", keywords: "cors origins allowed" },
              ],
            },
            {
              id: "sign-ups",
              label: "Sign-ups",
              icon: UserPlusIcon,
              description: "Who can join this instance and what they can make.",
              settings: [
                { id: "local-accounts", label: "Standalone accounts", keywords: "sign up password" },
                { id: "linked-accounts", label: "waifu.dev accounts", keywords: "linked sign in sign up" },
                { id: "linked-issuer", label: "Sign-in provider", keywords: "issuer openauth waifu.dev linked" },
                { id: "server-creation", label: "Who can create servers" },
                { id: "servers-per-account", label: "Servers per account" },
                { id: "agent-creation", label: "Who can make agents", keywords: "bots integrations" },
                { id: "shared-channels", label: "Shared channels", keywords: "share connect servers slack connect" },
                { id: "profile-effects", label: "Profile effects", keywords: "sparkles petals animation card decoration" },
              ],
            },
            {
              id: "sso",
              label: "Single sign-on",
              icon: BuildingIcon,
              description: "Let people sign in here through your organization's identity provider.",
              keywords: "sso saml oidc openid okta entra azure google workspace keycloak authentik identity provider",
              settings: [
                { id: "sso-accounts", label: "Single sign-on accounts", keywords: "sso sign up" },
                { id: "sso-protocol", label: "Identity provider", keywords: "saml oidc openid" },
                { id: "sso-domains", label: "Email domains", keywords: "sso allowed" },
                { id: "sso-test", label: "Test sign-in", keywords: "sso check" },
              ],
            },
            {
              id: "limits",
              label: "Limits",
              icon: GaugeIcon,
              description: "Caps every server starts with.",
              settings: [
                { id: "default-limits", label: "Default caps for every server", keywords: "members channels storage attachments" },
                { id: "picture-uploads", label: "Largest picture upload", keywords: "avatar banner icon image size" },
              ],
            },
            {
              id: "privacy",
              label: "Privacy",
              icon: ShieldCheckIcon,
              description: "What this instance tells Waifu Devs.",
              settings: [{ id: "telemetry", label: "Anonymous usage signal and reports", keywords: "telemetry analytics errors performance" }],
            },
            CALL_SECTION,
            MODERATION_SECTION,
            FEDERATION_SECTION,
          ],
        },
        {
          label: "Manage",
          sections: [
            {
              id: "accounts",
              label: "Accounts",
              icon: UsersIcon,
              description: "Everyone with an account here. Make admins, reset passwords, or turn an account off.",
              keywords: "users people disable ban reset password admin",
            },
            {
              id: "servers",
              label: "Servers",
              icon: ServerIcon,
              description: "Every server on this instance, whether you're in it or not.",
              keywords: "communities export backup delete caps usage storage",
            },
            {
              id: "announcement",
              label: "Announcement",
              icon: MegaphoneIcon,
              description: "A banner at the top of the app for everyone on this instance.",
              keywords: "banner maintenance notice news",
              settings: [
                { id: "announcement-text", label: "Announcement message" },
                { id: "announcement-tone", label: "Announcement tone", keywords: "urgent warning info" },
                { id: "announcement-ends", label: "When the announcement comes down", keywords: "expire end" },
              ],
            },
          ],
        },
      ]}
      footer={
        <SaveBar
          scope="screen"
          count={changed.length}
          saving={save.pending}
          error={save.error}
          onSave={() => commit(changed, [])}
          onDiscard={() => {
            if (saved) setDraft(clone(InstanceSettingsSchema, saved));
            save.setError(null);
          }}
        />
      }
    >
      {tab === "accounts" ? (
        <Accounts instanceKey={instanceKey} />
      ) : tab === "servers" ? (
        <Servers instanceKey={instanceKey} onLeave={() => onOpenChange(false)} />
      ) : tab === "announcement" ? (
        <Announcement instanceKey={instanceKey} />
      ) : loadError ? (
        <p className="text-sm text-muted-foreground first-letter:uppercase">{loadError}</p>
      ) : loading ? (
        <div className="flex flex-col gap-3">
          {[0, 1, 2].map((n) => (
            <div key={n} className="shimmer h-28 rounded-2xl" />
          ))}
        </div>
      ) : (
        <div className="flex flex-col">
          {tab === "general" && (
            <>
              <Setting id="name" title="Name" hint="Shown in the app and when people add this instance." defaultLabel={defaults.name} {...resetter("name")}>
                <Input value={draft.name} maxLength={64} onChange={(e) => patch((d) => (d.name = e.target.value))} className="h-10 rounded-xl" />
              </Setting>
              <Setting
                id="public-url"
                title="Public address"
                hint="The URL people use to reach this instance. Uploaded pictures are linked through it, so set it before people upload."
                defaultLabel={privateField ? HIDDEN_ADDRESS : defaults.publicUrl}
                delay={0.04}
                {...resetter("public_url")}
              >
                <div className="relative">
                  <LinkIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
                  <Input
                    value={draft.publicUrl}
                    type="url"
                    onChange={(e) => patch((d) => (d.publicUrl = e.target.value))}
                    className={cn("h-10 rounded-xl pl-9", privateField)}
                  />
                </div>
              </Setting>
              <Setting id="web" title="Web app" delay={0.08} defaultLabel={defaults.web ? "on" : "off"} {...resetter("web")}>
                <Toggle
                  checked={draft.web}
                  disabled={!config.startup?.webBuiltIn}
                  onChange={(web) => patch((d) => (d.web = web))}
                  label="Open the app at this address"
                  hint={
                    config.startup?.webBuiltIn
                      ? "Turned off, people can still use this instance from the app on another fuwa instance."
                      : "This build of fuwa doesn't include the web app."
                  }
                />
              </Setting>
              <Origins
                draft={draft}
                patch={patch}
                defaultLabel={privateField ? HIDDEN_ADDRESS : defaults.allowedOrigins.join(", ")}
                reset={resetter("allowed_origins")}
              />
              <Startup config={config} />
            </>
          )}
          {tab === "sign-ups" && (
            <>
              <Setting
                id="local-accounts"
                title="Standalone accounts"
                hint="A username and password kept on this instance only."
                defaultLabel={ACCOUNTS_LABEL[defaults.localAccounts]}
                {...resetter("local_accounts")}
              >
                <Choice
                  value={draft.localAccounts}
                  onChange={(v) => patch((d) => (d.localAccounts = v))}
                  options={[
                    { value: LocalAccounts.OPEN, label: "Open", hint: "Anyone can sign up.", icon: <DoorOpenIcon className="size-4" /> },
                    { value: LocalAccounts.CLOSED, label: "Closed", hint: "Existing accounts only.", icon: <DoorClosedIcon className="size-4" /> },
                    {
                      value: LocalAccounts.OFF,
                      label: "Off",
                      hint: "No standalone accounts.",
                      icon: <LockIcon className="size-4" />,
                      disabled:
                        draft.localAccounts === LocalAccounts.OFF || linkedWorks(draft) || ssoWorks(draft)
                          ? undefined
                          : "Needs waifu.dev sign-in or single sign-on working first.",
                    },
                  ]}
                />
                <Notice show={draft.localAccounts === LocalAccounts.OFF && saved?.localAccounts !== LocalAccounts.OFF && hasPasswordHere}>
                  You sign in here with a password. Once you sign out, you'll need another way in here to get back in.
                </Notice>
              </Setting>
              <Setting
                id="linked-accounts"
                title="waifu.dev accounts"
                hint="People sign in with their waifu.dev account, and get an account here the first time."
                defaultLabel={ACCOUNTS_LABEL[defaults.linkedAccounts]}
                delay={0.04}
                {...resetter("linked_accounts")}
              >
                <Choice
                  value={draft.linkedAccounts}
                  onChange={(v) => patch((d) => (d.linkedAccounts = v))}
                  options={[
                    { value: LinkedAccounts.OPEN, label: "Open", hint: "Anyone with waifu.dev.", icon: <Flower2Icon className="size-4" /> },
                    { value: LinkedAccounts.CLOSED, label: "Closed", hint: "Linked accounts only.", icon: <DoorClosedIcon className="size-4" /> },
                    {
                      value: LinkedAccounts.OFF,
                      label: "Off",
                      hint: "No waifu.dev sign-in.",
                      icon: <LockIcon className="size-4" />,
                      disabled:
                        draft.linkedAccounts === LinkedAccounts.OFF || draft.localAccounts !== LocalAccounts.OFF || ssoWorks(draft)
                          ? undefined
                          : "Needs standalone accounts on first.",
                    },
                  ]}
                />
                <Notice show={draft.linkedAccounts !== LinkedAccounts.OFF && !canReturnTo(draft.publicUrl)}>
                  waifu.dev can only send people back to an https address. Set the public address under General to turn this on.
                </Notice>
              </Setting>
              <Setting
                id="linked-issuer"
                title="Sign-in provider"
                hint="The OpenAuth issuer waifu.dev sign-ins go through. Accounts already linked stay tied to the one they came from."
                defaultLabel={defaults.linkedIssuer}
                delay={0.08}
                {...resetter("linked_issuer")}
              >
                <div className="relative">
                  <Flower2Icon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
                  <Input
                    value={draft.linkedIssuer}
                    type="url"
                    placeholder={WAIFU_DEV_ISSUER}
                    onChange={(e) => patch((d) => (d.linkedIssuer = e.target.value))}
                    className="h-10 rounded-xl pl-9"
                  />
                </div>
              </Setting>
              <Setting
                id="server-creation"
                title="Who can create servers"
                defaultLabel={CREATION_LABEL[defaults.serverCreation]}
                delay={0.12}
                {...resetter("server_creation")}
              >
                <Choice
                  value={draft.serverCreation}
                  onChange={(v) => patch((d) => (d.serverCreation = v))}
                  options={[
                    { value: ServerCreation.EVERYONE, label: "Everyone", hint: "Any signed-in account.", icon: <UsersIcon className="size-4" /> },
                    { value: ServerCreation.ADMINS, label: "Admins", hint: "Instance admins only.", icon: <CrownIcon className="size-4" /> },
                    { value: ServerCreation.DISABLED, label: "Nobody", hint: "No new servers.", icon: <BanIcon className="size-4" /> },
                  ]}
                />
              </Setting>
              <Setting
                id="servers-per-account"
                title="Servers per account"
                hint="How many servers one account may own."
                defaultLabel={count(defaults.serversPerAccount)}
                delay={0.16}
                {...resetter("servers_per_account")}
              >
                <Cap label="Up to" value={draft.serversPerAccount} onChange={(v) => patch((d) => (d.serversPerAccount = v))} />
              </Setting>
              <Setting
                id="agent-creation"
                title="Who can make agents"
                hint="Agents are accounts programs drive, such as bots. Agents already made keep working."
                defaultLabel={AGENT_CREATION_LABEL[defaults.agentCreation]}
                delay={0.2}
                {...resetter("agent_creation")}
              >
                <Choice
                  value={draft.agentCreation}
                  onChange={(v) => patch((d) => (d.agentCreation = v))}
                  options={[
                    { value: AgentCreation.EVERYONE, label: "Everyone", hint: "Any signed-in person.", icon: <BotIcon className="size-4" /> },
                    { value: AgentCreation.ADMINS, label: "Admins", hint: "Instance admins only.", icon: <CrownIcon className="size-4" /> },
                    { value: AgentCreation.DISABLED, label: "Nobody", hint: "No new agents.", icon: <BanIcon className="size-4" /> },
                  ]}
                />
              </Setting>
              <Setting
                id="shared-channels"
                title="Shared channels"
                defaultLabel={defaults.sharedChannels ? "on" : "off"}
                delay={0.24}
                {...resetter("shared_channels")}
              >
                <Toggle
                  checked={draft.sharedChannels}
                  onChange={(on) => patch((d) => (d.sharedChannels = on))}
                  label="Servers can share channels with each other"
                  hint="Admins of two servers here can show one channel in both. Turned off, nobody can start a new one; channels already shared stay until either side ends them."
                />
              </Setting>
              <Setting
                id="profile-effects"
                title="Profile effects"
                defaultLabel={defaults.profileEffects ? "on" : "off"}
                delay={0.28}
                {...resetter("profile_effects")}
              >
                <Toggle
                  checked={draft.profileEffects}
                  onChange={(on) => patch((d) => (d.profileEffects = on))}
                  label="People can put an effect on their profile card"
                  hint="Petals, stars and the like, drawn by the app from your theme's colors. Turned off, nobody's shows, and everyone's pick comes back when it's on again."
                />
              </Setting>
            </>
          )}
          {tab === "sso" && (
            <>
              <Setting
                id="sso-accounts"
                title="Single sign-on accounts"
                hint="People sign in through the identity provider below, and get an account here the first time."
                defaultLabel={ACCOUNTS_LABEL[defaults.ssoAccounts]}
                {...resetter("sso_accounts")}
              >
                <Choice
                  value={draft.ssoAccounts === SsoAccounts.UNSPECIFIED ? SsoAccounts.OFF : draft.ssoAccounts}
                  onChange={(v) => patch((d) => (d.ssoAccounts = v))}
                  options={[
                    {
                      value: SsoAccounts.OPEN,
                      label: "Open",
                      hint: "Anyone the provider lets in.",
                      icon: <BuildingIcon className="size-4" />,
                      disabled: providerReady(draft.ssoProvider) ? undefined : "Set up the identity provider first.",
                    },
                    {
                      value: SsoAccounts.CLOSED,
                      label: "Closed",
                      hint: "Accounts made before only.",
                      icon: <DoorClosedIcon className="size-4" />,
                      disabled: providerReady(draft.ssoProvider) ? undefined : "Set up the identity provider first.",
                    },
                    {
                      value: SsoAccounts.OFF,
                      label: "Off",
                      hint: "No single sign-on.",
                      icon: <LockIcon className="size-4" />,
                      disabled:
                        draft.ssoAccounts === SsoAccounts.OFF || draft.localAccounts !== LocalAccounts.OFF || linkedWorks(draft)
                          ? undefined
                          : "Needs another way in first.",
                    },
                  ]}
                />
                <Notice show={draft.ssoAccounts !== SsoAccounts.OFF && draft.ssoAccounts !== SsoAccounts.UNSPECIFIED && !canReturnTo(draft.publicUrl)}>
                  Identity providers send people back to this instance's public address, which has to be https. Set it under General.
                </Notice>
              </Setting>
              <IdentityProviderForm
                value={fullProvider(draft.ssoProvider)}
                onChange={patchProvider}
                serviceProvider={config.ssoServiceProvider}
                offHint="No provider set up."
                test={{
                  onTest: () => void test.go(inst!.url, window.location.pathname, { key: instanceKey }),
                  pending: test.pending,
                  error: test.error,
                  blocked: changed.includes("sso_provider")
                    ? "Save first; the test uses the saved provider."
                    : !providerReady(saved?.ssoProvider)
                      ? "Fill it in and save first."
                      : undefined,
                }}
              />
            </>
          )}
          {tab === "limits" && (
            <>
              <Setting
                id="default-limits"
                title="Default caps for every server"
                hint="A server can get its own caps from its settings. With a cap off, it's unlimited."
                defaultLabel={[
                  `${count(defaults.defaultLimits?.members)} members`,
                  `${count(defaults.defaultLimits?.channels)} channels`,
                  `${size(defaults.defaultLimits?.storageBytes)} storage`,
                  `${size(defaults.defaultLimits?.attachmentBytes)} files`,
                  `${count(defaults.defaultLimits?.emojis)} emoji`,
                  `${size(defaults.defaultLimits?.recordingBytes)} recordings`,
                ].join(", ")}
                {...resetter(
                  "default_limits.members",
                  "default_limits.channels",
                  "default_limits.storage_bytes",
                  "default_limits.attachment_bytes",
                  "default_limits.emojis",
                  "default_limits.recording_bytes",
                )}
              >
                <div className="flex flex-col gap-3">
                  <Cap label="Members" value={draft.defaultLimits?.members} onChange={(v) => patch((d) => (d.defaultLimits!.members = v))} />
                  <Cap label="Channels" value={draft.defaultLimits?.channels} onChange={(v) => patch((d) => (d.defaultLimits!.channels = v))} />
                  <Cap label="Storage" bytes value={draft.defaultLimits?.storageBytes} onChange={(v) => patch((d) => (d.defaultLimits!.storageBytes = v))} />
                  <Cap label="Files" bytes value={draft.defaultLimits?.attachmentBytes} onChange={(v) => patch((d) => (d.defaultLimits!.attachmentBytes = v))} />
                  <Cap label="Emoji" value={draft.defaultLimits?.emojis} onChange={(v) => patch((d) => (d.defaultLimits!.emojis = v))} />
                  <Cap label="Recordings" bytes value={draft.defaultLimits?.recordingBytes} onChange={(v) => patch((d) => (d.defaultLimits!.recordingBytes = v))} />
                </div>
              </Setting>
              <Setting
                id="picture-uploads"
                title="Largest picture upload"
                hint="Avatars, banners and server icons. The app crops pictures and saves them small, so only GIFs, which go up as they are, get near a few megabytes."
                defaultLabel={size(defaults.pictureUploadBytes)}
                delay={0.04}
                {...resetter("picture_upload_bytes")}
              >
                <Cap label="Up to" bytes value={draft.pictureUploadBytes} onChange={(v) => patch((d) => (d.pictureUploadBytes = v))} />
              </Setting>
              <Setting
                id="picture-uploads-per-day"
                title="Pictures per day"
                hint="How much one account may upload in a day (UTC), so nobody can fill this instance's disk."
                defaultLabel={size(defaults.pictureUploadBytesPerDay)}
                delay={0.08}
                {...resetter("picture_upload_bytes_per_day")}
              >
                <Cap
                  label="Up to"
                  bytes
                  value={draft.pictureUploadBytesPerDay}
                  onChange={(v) => patch((d) => (d.pictureUploadBytesPerDay = v))}
                />
              </Setting>
              <Setting
                id="poll-votes-per-minute"
                title="Poll votes per minute"
                hint="How many times one account may vote, change or take back a vote in polls in a minute. Every vote is a live update to everyone in the channel."
                defaultLabel={defaults.pollVotesPerMinute === undefined ? "no limit" : `${defaults.pollVotesPerMinute.toLocaleString()} a minute`}
                delay={0.12}
                {...resetter("poll_votes_per_minute")}
              >
                <Cap label="Up to" placeholder="30" value={draft.pollVotesPerMinute} onChange={(v) => patch((d) => (d.pollVotesPerMinute = v))} />
              </Setting>
            </>
          )}
          {tab === "calls" && <CallSettings config={config} draft={draft} defaults={defaults} patch={patch} resetter={resetter} />}
          {tab === "moderation" && saved && (
            <ModerationSettings instanceKey={instanceKey} draft={draft} saved={saved} defaults={defaults} patch={patch} resetter={resetter} />
          )}
          {tab === "federation" && saved && (
            <FederationSettings instanceKey={instanceKey} draft={draft} saved={saved} defaults={defaults} patch={patch} resetter={resetter} />
          )}
          {tab === "privacy" && (
            <>
              <Setting id="telemetry" title="Anonymous usage signal and reports" defaultLabel={defaults.telemetry ? "on" : "off"} {...resetter("telemetry")}>
                <Toggle
                  checked={draft.telemetry}
                  onChange={(telemetry) => patch((d) => (d.telemetry = telemetry))}
                  label="Send the usage signal daily and error reports hourly"
                  hint="Helps Waifu Devs see how fuwa is used and fix what breaks. Counts only: no names, messages, ids or addresses. Off, apps on this instance send no reports either."
                />
                <ul className="grid gap-1.5 text-xs text-muted-foreground sm:grid-cols-2">
                  {["How many accounts, servers, channels and messages", "Storage used, in bytes", "Which account and server options are on", "fuwa version, OS and a random install id", "Kinds of errors and where, and how long requests took (server and apps)"].map(
                    (line, n) => (
                      <motion.li
                        key={line}
                        initial={{ opacity: 0, x: -8 }}
                        animate={{ opacity: 1, x: 0 }}
                        transition={{ ...SPRING, delay: 0.1 + n * 0.05 }}
                        className="flex items-center gap-2"
                      >
                        <ShieldCheckIcon className="size-3.5 shrink-0 text-primary" /> {line}
                      </motion.li>
                    ),
                  )}
                </ul>
                <a
                  href="https://github.com/waifu-devs/fuwa#the-anonymous-usage-signal"
                  target="_blank"
                  rel="noreferrer"
                  className="text-xs font-bold text-primary underline-offset-4 hover:underline"
                >
                  Every field it sends
                </a>
              </Setting>
            </>
          )}
        </div>
      )}
    </SettingsScreen>
  );
}

const ACCOUNTS_LABEL: Record<number, string> = {
  [LocalAccounts.OPEN]: "open",
  [LocalAccounts.CLOSED]: "closed",
  [LocalAccounts.OFF]: "off",
};

/** Whether waifu.dev sign-in would work with these settings: on, with an https public address. */
const linkedWorks = (s: InstanceSettings) => s.linkedAccounts !== LinkedAccounts.OFF && canReturnTo(s.publicUrl);

/** Whether single sign-on would work with these settings: on, set up, with an https public address. */
const ssoWorks = (s: InstanceSettings) =>
  (s.ssoAccounts === SsoAccounts.OPEN || s.ssoAccounts === SsoAccounts.CLOSED) && providerReady(s.ssoProvider) && canReturnTo(s.publicUrl);

/** A heads-up under a setting, sliding in while it applies. */
function Notice({ show, children }: { show: boolean; children: ReactNode }) {
  return (
    <AnimatePresence initial={false}>
      {show && (
        <motion.p
          initial={{ opacity: 0, height: 0 }}
          animate={{ opacity: 1, height: "auto" }}
          exit={{ opacity: 0, height: 0 }}
          transition={SPRING}
          className="overflow-hidden rounded-xl bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300"
        >
          {children}
        </motion.p>
      )}
    </AnimatePresence>
  );
}

const AGENT_CREATION_LABEL: Record<number, string> = {
  [AgentCreation.EVERYONE]: "everyone",
  [AgentCreation.ADMINS]: "admins",
  [AgentCreation.DISABLED]: "nobody",
};

const CREATION_LABEL: Record<number, string> = {
  [ServerCreation.EVERYONE]: "everyone",
  [ServerCreation.ADMINS]: "admins",
  [ServerCreation.DISABLED]: "nobody",
};

/** Settings copied by a function of their own, beside the switch below. */
const COPIED = [
  ...CALL_FIELDS,
  { path: "shared_channels", copy: (into: InstanceSettings, from: InstanceSettings) => (into.sharedChannels = from.sharedChannels) },
  { path: "profile_effects", copy: (into: InstanceSettings, from: InstanceSettings) => (into.profileEffects = from.profileEffects) },
];

/** Copies the named settings from one draft into another. */
function mergeFields(into: InstanceSettings, from: InstanceSettings, paths: string[]) {
  for (const path of paths) {
    COPIED.find((f) => f.path === path)?.copy(into, from);
    MODERATION_FIELDS.find((f) => f.path === path)?.copy(into, from);
    FEDERATION_FIELDS.find((f) => f.path === path)?.copy(into, from);
    switch (path) {
      case "name":
        into.name = from.name;
        break;
      case "public_url":
        into.publicUrl = from.publicUrl;
        break;
      case "allowed_origins":
        into.allowedOrigins = [...from.allowedOrigins];
        break;
      case "local_accounts":
        into.localAccounts = from.localAccounts;
        break;
      case "linked_accounts":
        into.linkedAccounts = from.linkedAccounts;
        break;
      case "linked_issuer":
        into.linkedIssuer = from.linkedIssuer;
        break;
      case "sso_accounts":
        into.ssoAccounts = from.ssoAccounts;
        break;
      case "sso_provider":
        into.ssoProvider = fullProvider(from.ssoProvider);
        break;
      case "server_creation":
        into.serverCreation = from.serverCreation;
        break;
      case "servers_per_account":
        into.serversPerAccount = from.serversPerAccount;
        break;
      case "agent_creation":
        into.agentCreation = from.agentCreation;
        break;
      case "telemetry":
        into.telemetry = from.telemetry;
        break;
      case "web":
        into.web = from.web;
        break;
      case "picture_upload_bytes":
        into.pictureUploadBytes = from.pictureUploadBytes;
        break;
      case "picture_upload_bytes_per_day":
        into.pictureUploadBytesPerDay = from.pictureUploadBytesPerDay;
        break;
      case "poll_votes_per_minute":
        into.pollVotesPerMinute = from.pollVotesPerMinute;
        break;
      default: {
        // Settings copied above by their own pages' functions.
        if (!path.startsWith("default_limits.")) break;
        const key = path.replace("default_limits.", "") as "members" | "channels" | "storage_bytes" | "attachment_bytes" | "emojis" | "recording_bytes";
        const field = key === "storage_bytes" ? "storageBytes" : key === "attachment_bytes" ? "attachmentBytes" : key === "recording_bytes" ? "recordingBytes" : key;
        into.defaultLimits ??= create(ServerLimitsSchema);
        into.defaultLimits[field] = from.defaultLimits?.[field];
      }
    }
  }
  return into;
}

/** Which sites' pages may call this instance: any (so every fuwa app works), or a list. */
function Origins({
  draft,
  patch,
  defaultLabel,
  reset,
}: {
  draft: InstanceSettings;
  patch: (fn: (d: InstanceSettings) => void) => void;
  defaultLabel: string;
  reset: { changed: boolean; onReset: () => void; resetting: boolean };
}) {
  const any = draft.allowedOrigins.includes("*");
  const privateField = usePrivateField();
  const [list, setList] = useState(any ? "" : draft.allowedOrigins.join("\n"));
  useEffect(() => {
    if (!any) setList((current) => (current.split("\n").map((o) => o.trim()).filter(Boolean).join("\n") === draft.allowedOrigins.join("\n") ? current : draft.allowedOrigins.join("\n")));
  }, [any, draft.allowedOrigins]);
  return (
    <Setting
      id="origins"
      title="Sites that can connect"
      hint="Web pages on other sites, such as the fuwa app on another instance, that may use this one from a browser. Signing in with waifu.dev from another site only works for sites listed here by name."
      defaultLabel={defaultLabel === "*" ? "any site" : defaultLabel}
      delay={0.12}
      {...reset}
    >
      <Choice
        value={any ? "any" : "list"}
        onChange={(v) =>
          patch((d) => {
            d.allowedOrigins = v === "any" ? ["*"] : list.split("\n").map((o) => o.trim()).filter(Boolean);
            if (v === "list" && d.allowedOrigins.length === 0) d.allowedOrigins = [window.location.origin];
          })
        }
        options={[
          { value: "any", label: "Any site", hint: "Every fuwa app can connect.", icon: <GlobeIcon className="size-4" /> },
          { value: "list", label: "Only these", hint: "Other apps are blocked.", icon: <LockIcon className="size-4" /> },
        ]}
      />
      <motion.div initial={false} animate={{ height: any ? 0 : "auto", opacity: any ? 0 : 1 }} transition={SPRING} className="overflow-hidden">
        <Textarea
          rows={3}
          value={list}
          placeholder={"https://fuwa.waifu.dev\nhttps://chat.example.com"}
          onChange={(e) => {
            setList(e.target.value);
            patch((d) => (d.allowedOrigins = e.target.value.split("\n").map((o) => o.trim()).filter(Boolean)));
          }}
          className={cn("rounded-xl font-mono text-xs", privateField)}
        />
        <p className="mt-1.5 text-xs text-muted-foreground">One per line, like https://chat.example.com. This page (<Private text={window.location.origin} />) always works when served by this instance.</p>
      </motion.div>
    </Setting>
  );
}

/** How the process was started: read-only, set by whoever runs the instance. */
function Startup({ config }: { config: InstanceConfig }) {
  const s = config.startup;
  if (!s) return null;
  const facts: { label: string; value: ReactNode; good?: boolean }[] = [
    { label: "Version", value: s.version },
    { label: "Port", value: s.port },
    { label: "Encryption at rest", value: s.encryption ? "on" : "off", good: s.encryption },
    { label: "Admin token", value: s.adminToken ? "set" : "not set" },
    { label: "Hosting", value: s.hosted ? "Waifu Devs" : "self-hosted" },
  ];
  return (
    <motion.section
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ ...SPRING, delay: 0.16 }}
      className="mt-2 rounded-2xl border border-dashed p-4"
    >
      <h3 className="text-sm font-extrabold">Set when the instance starts</h3>
      <p className="mt-0.5 text-xs text-muted-foreground">Changed only by whoever runs it, through FUWA_* environment variables.</p>
      <div className="mt-3 flex flex-wrap gap-2">
        {facts.map((f, n) => (
          <motion.span
            key={f.label}
            initial={{ opacity: 0, scale: 0.9 }}
            animate={{ opacity: 1, scale: 1 }}
            transition={{ ...SPRING, delay: 0.2 + n * 0.04 }}
            className="rounded-full border bg-muted/50 px-3 py-1 text-xs"
          >
            <span className="text-muted-foreground">{f.label}</span>{" "}
            <b className={cn(f.good && "text-primary")}>{f.value}</b>
          </motion.span>
        ))}
      </div>
    </motion.section>
  );
}

