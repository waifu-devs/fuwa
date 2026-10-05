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
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useReducer, useState, type ComponentProps, type ReactNode } from "react";
import { NewerRelease } from "@/components/settings/instance/NewerRelease";
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
import { formatBytes, type Lang } from "@/lib/format";
import { type I18n, type Key, T, useI18n } from "@/i18n/react";
import { canReturnTo, WAIFU_DEV_ISSUER } from "@/lib/linked";
import { HIDDEN_ADDRESS } from "@/lib/streamer";
import { cn } from "@/lib/utils";
import { Cap, Choice, SaveBar, Setting, SPRING, Toggle } from "./controls";
import { fullProvider, IdentityProviderForm, providerFingerprint } from "./IdentityProviderForm";
import { providerReady } from "@/lib/sso";
import type { IdentityProvider } from "@/gen/fuwa/v1/sso_pb";
import { Accounts } from "./instance/Accounts";
import { Announcement } from "./instance/Announcement";
import { CALL_FIELDS, callSection, CallSettings } from "./instance/Calls";
import { GifSettings } from "./instance/Gifs";
import { GIF_FIELDS, gifSection } from "./instance/gif-fields";
import { FEDERATION_FIELDS, federationSection, FederationSettings } from "./instance/Federation";
import { MODERATION_FIELDS, moderationSection, ModerationSettings } from "./instance/Moderation";
import { Servers } from "./instance/Servers";
import { SettingsScreen, type SettingsGroup } from "./SettingsScreen";

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
  { path: "mcp", get: (s) => s.mcp },
  { path: "profile_effects", get: (s) => s.profileEffects },
  { path: "rich_presence", get: (s) => s.richPresence },
  { path: "servers_per_account", get: (s) => s.serversPerAccount },
  { path: "default_limits.members", get: (s) => s.defaultLimits?.members },
  { path: "default_limits.channels", get: (s) => s.defaultLimits?.channels },
  { path: "default_limits.storage_bytes", get: (s) => s.defaultLimits?.storageBytes },
  { path: "default_limits.attachment_bytes", get: (s) => s.defaultLimits?.attachmentBytes },
  { path: "default_limits.emojis", get: (s) => s.defaultLimits?.emojis },
  { path: "default_limits.recording_bytes", get: (s) => s.defaultLimits?.recordingBytes },
  { path: "picture_upload_bytes", get: (s) => s.pictureUploadBytes },
  { path: "picture_upload_bytes_per_day", get: (s) => s.pictureUploadBytesPerDay },
  { path: "attachment_upload_bytes", get: (s) => s.attachmentUploadBytes },
  { path: "attachment_upload_bytes_per_day", get: (s) => s.attachmentUploadBytesPerDay },
  { path: "voice_message_seconds", get: (s) => s.voiceMessageSeconds },
  { path: "voice_message_bytes", get: (s) => s.voiceMessageBytes },
  { path: "voice_message_bytes_per_day", get: (s) => s.voiceMessageBytesPerDay },
  { path: "poll_votes_per_minute", get: (s) => s.pollVotesPerMinute },
  { path: "commands_per_minute", get: (s) => s.commandsPerMinute },
  { path: "telemetry", get: (s) => s.telemetry },
  { path: "web", get: (s) => s.web },
  ...CALL_FIELDS,
  ...MODERATION_FIELDS,
  ...FEDERATION_FIELDS,
  ...GIF_FIELDS,
];

const changedPaths = (draft: InstanceSettings, saved: InstanceSettings) =>
  FIELDS.filter((f) => f.get(draft) !== f.get(saved)).map((f) => f.path);

const count = (lang: I18n, n: bigint | undefined) => (n === undefined ? lang.t("instancesettings.shared.noLimit") : lang.number(Number(n)));
const size = (lang: Lang, n: bigint | undefined) => (n === undefined ? lang.t("instancesettings.shared.noLimit") : formatBytes(lang, Number(n)));
const onOff = (t: I18n["t"], on: boolean | undefined) => t(on ? "instancesettings.shared.on" : "instancesettings.shared.off");

/** What a setting's Reset needs: whether it's changed here, and how to put it back. */
type Reset = { changed: boolean; onReset: () => void; resetting: boolean };
type Patch = (fn: (d: InstanceSettings) => void) => void;
type Resetter = (...paths: string[]) => Reset;

type DraftAction =
  /** An edit, made on a copy of the latest draft. */
  | { type: "patch"; fn: (d: InstanceSettings) => void }
  /** Starts again from these settings. */
  | { type: "take"; settings: InstanceSettings }
  /** Just saved: the stored settings, keeping unsaved edits to the ones not in `done`. */
  | { type: "saved"; settings: InstanceSettings; before: InstanceSettings | undefined; done: ReadonlySet<string> };

function draftReducer(d: InstanceSettings | null, action: DraftAction): InstanceSettings | null {
  switch (action.type) {
    case "patch": {
      if (!d) return d;
      const next = clone(InstanceSettingsSchema, d);
      next.defaultLimits ??= create(ServerLimitsSchema);
      action.fn(next);
      return next;
    }
    case "take":
      return clone(InstanceSettingsSchema, action.settings);
    case "saved": {
      // Keep unsaved edits to other settings; take the stored value for the rest.
      const fresh = clone(InstanceSettingsSchema, action.settings);
      if (!d || !action.before) return fresh;
      const pending = changedPaths(d, action.before).filter((p) => !action.done.has(p));
      return pending.length ? mergeFields(fresh, d, pending) : fresh;
    }
  }
}

/**
 * The settings as stored and as being edited, loaded afresh (back on General)
 * each time the screen opens.
 */
function useInstanceSettings(instanceKey: string, open: boolean) {
  const [config, setConfig] = useState<InstanceConfig | null>(null);
  const [draft, dispatch] = useReducer(draftReducer, null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tab, setTab] = useState("general");
  const save = useAction(updateSettings);

  // Opening (or switching instance while open) starts over before anything stale shows.
  const opened = open ? instanceKey : null;
  const [shown, setShown] = useState<string | null>(null);
  if (opened !== shown) {
    setShown(opened);
    if (opened !== null) {
      setConfig(null);
      setLoadError(null);
      setTab("general");
    }
  }

  useEffect(() => {
    if (!open) return;
    run(getSettings(instanceKey)).then(
      (c) => {
        setConfig(c);
        dispatch({ type: "take", settings: c.settings! });
      },
      (e) => setLoadError(e.message),
    );
  }, [open, instanceKey]);

  const saved = config?.settings;
  const changed = saved && draft ? changedPaths(draft, saved) : [];
  const overridden = new Set(config?.overridden ?? []);

  const patch: Patch = (fn) => {
    dispatch({ type: "patch", fn });
    save.setError(null);
  };

  async function commit(update: string[], reset: string[]) {
    if (!draft) return;
    const next = await save.go(instanceKey, draft, update, reset);
    if (!next) return;
    setConfig(next);
    dispatch({ type: "saved", settings: next.settings!, before: saved, done: new Set([...update, ...reset]) });
  }

  const resetter: Resetter = (...paths) => ({
    changed: paths.some((p) => overridden.has(p)),
    onReset: () => commit([], paths),
    resetting: save.pending,
  });

  const discard = () => {
    if (saved) dispatch({ type: "take", settings: saved });
    save.setError(null);
  };

  return { config, draft, loadError, tab, setTab, save, saved, changed, patch, commit, resetter, discard };
}

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
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const { config, draft, loadError, tab, setTab, save, saved, changed, patch, commit, resetter, discard } = useInstanceSettings(instanceKey, open);
  const test = useAction(startSsoSignIn);
  const name = inst?.node?.name ?? instanceKey;

  return (
    <SettingsScreen
      open={open}
      onOpenChange={onOpenChange}
      title={name}
      subtitle={t("instancesettings.nav.subtitle")}
      section={tab}
      onSectionChange={setTab}
      groups={settingsGroups(t, name)}
      footer={<SaveBar scope="screen" count={changed.length} saving={save.pending} error={save.error} onSave={() => commit(changed, [])} onDiscard={discard} />}
    >
      <SettingsBody
        tab={tab}
        instanceKey={instanceKey}
        onLeave={() => onOpenChange(false)}
        loadError={loadError}
        config={config}
        draft={draft}
        saved={saved}
        changed={changed}
        patch={patch}
        resetter={resetter}
        test={test}
      />
    </SettingsScreen>
  );
}

/** What the screen shows for the page picked: a page that manages things, or its settings once they've loaded. */
function SettingsBody({
  tab,
  instanceKey,
  onLeave,
  loadError,
  config,
  draft,
  ...props
}: Omit<ComponentProps<typeof SettingsTab>, "config" | "draft" | "defaults"> & {
  onLeave: () => void;
  loadError: string | null;
  config: InstanceConfig | null;
  draft: InstanceSettings | null;
}) {
  if (tab === "accounts") return <Accounts instanceKey={instanceKey} />;
  if (tab === "servers") return <Servers instanceKey={instanceKey} onLeave={onLeave} />;
  if (tab === "announcement") return <Announcement instanceKey={instanceKey} />;
  if (loadError) return <p className="text-sm text-muted-foreground first-letter:uppercase">{loadError}</p>;
  if (!config || !draft || !config.defaults) {
    return (
      <div className="flex flex-col gap-3">
        {[0, 1, 2].map((n) => (
          <div key={n} className="shimmer h-28 rounded-2xl" />
        ))}
      </div>
    );
  }
  return (
    <div className="flex flex-col">
      <SettingsTab tab={tab} instanceKey={instanceKey} config={config} draft={draft} defaults={config.defaults} {...props} />
    </div>
  );
}

/** The menu: the instance's settings, then what an admin manages. */
function settingsGroups(t: I18n["t"], name: string): SettingsGroup[] {
  return [
    {
      label: t("instancesettings.nav.instance"),
      sections: [
        {
          id: "general",
          label: t("instancesettings.nav.general"),
          icon: SlidersHorizontalIcon,
          description: t("instancesettings.nav.generalAbout", { name }),
          settings: [
            { id: "name", label: t("instancesettings.nav.name") },
            { id: "public-url", label: t("instancesettings.nav.publicUrl"), keywords: "url domain" },
            { id: "web", label: t("instancesettings.nav.web") },
            { id: "origins", label: t("instancesettings.nav.origins"), keywords: "cors origins allowed" },
          ],
        },
        {
          id: "sign-ups",
          label: t("instancesettings.nav.signUps"),
          icon: UserPlusIcon,
          description: t("instancesettings.nav.signUpsAbout"),
          settings: [
            { id: "local-accounts", label: t("instancesettings.nav.localAccounts"), keywords: "sign up password" },
            { id: "linked-accounts", label: t("instancesettings.nav.linkedAccounts"), keywords: "linked sign in sign up" },
            { id: "linked-issuer", label: t("instancesettings.nav.linkedIssuer"), keywords: "issuer openauth waifu.dev linked" },
            { id: "server-creation", label: t("instancesettings.nav.serverCreation") },
            { id: "servers-per-account", label: t("instancesettings.nav.serversPerAccount") },
            { id: "agent-creation", label: t("instancesettings.nav.agentCreation"), keywords: "bots integrations" },
            { id: "mcp", label: t("instancesettings.nav.mcp"), keywords: "mcp claude ai model context protocol" },
            { id: "shared-channels", label: t("serversettings.nav.shared"), keywords: "share connect servers slack connect" },
            { id: "profile-effects", label: t("instancesettings.nav.profileEffects"), keywords: "sparkles petals animation card decoration" },
            { id: "rich-presence", label: t("instancesettings.nav.richPresence"), keywords: "activity playing game status discord presence" },
          ],
        },
        {
          id: "sso",
          label: t("serversettings.nav.sso"),
          icon: BuildingIcon,
          description: t("instancesettings.nav.ssoAbout"),
          keywords: "sso saml oidc openid okta entra azure google workspace keycloak authentik identity provider",
          settings: [
            { id: "sso-accounts", label: t("instancesettings.nav.ssoAccounts"), keywords: "sso sign up" },
            { id: "sso-protocol", label: t("serversettings.nav.ssoProtocol"), keywords: "saml oidc openid" },
            { id: "sso-domains", label: t("serversettings.nav.ssoDomains"), keywords: "sso allowed" },
            { id: "sso-test", label: t("instancesettings.nav.ssoTest"), keywords: "sso check" },
          ],
        },
        {
          id: "limits",
          label: t("serversettings.nav.limits"),
          icon: GaugeIcon,
          description: t("instancesettings.nav.limitsAbout"),
          settings: [
            { id: "default-limits", label: t("instancesettings.nav.defaultLimits"), keywords: "members channels storage attachments" },
            { id: "picture-uploads", label: t("instancesettings.nav.pictureUploads"), keywords: "avatar banner icon image size" },
          ],
        },
        {
          id: "privacy",
          label: t("instancesettings.nav.privacy"),
          icon: ShieldCheckIcon,
          description: t("instancesettings.nav.privacyAbout"),
          settings: [{ id: "telemetry", label: t("instancesettings.nav.telemetry"), keywords: "telemetry analytics errors performance" }],
        },
        callSection(t),
        moderationSection(t),
        federationSection(t),
        gifSection(t),
      ],
    },
    {
      label: t("instancesettings.nav.manage"),
      sections: [
        {
          id: "accounts",
          label: t("instancesettings.nav.accounts"),
          icon: UsersIcon,
          description: t("instancesettings.nav.accountsAbout"),
          keywords: "users people disable ban reset password admin",
        },
        {
          id: "servers",
          label: t("instancesettings.nav.servers"),
          icon: ServerIcon,
          description: t("instancesettings.nav.serversAbout"),
          keywords: "communities export backup delete caps usage storage",
        },
        {
          id: "announcement",
          label: t("instancesettings.nav.announcement"),
          icon: MegaphoneIcon,
          description: t("instancesettings.nav.announcementAbout"),
          keywords: "banner maintenance notice news",
          settings: [
            { id: "announcement-text", label: t("instancesettings.nav.announcementText") },
            { id: "announcement-tone", label: t("instancesettings.nav.announcementTone"), keywords: "urgent warning info" },
            { id: "announcement-ends", label: t("instancesettings.nav.announcementEnds"), keywords: "expire end" },
          ],
        },
      ],
    },
  ];
}

type TabProps = {
  draft: InstanceSettings;
  defaults: InstanceSettings;
  patch: Patch;
  resetter: Resetter;
};

/** The settings on one page of the menu, once they've loaded. */
function SettingsTab({
  tab,
  instanceKey,
  config,
  saved,
  changed,
  test,
  ...props
}: TabProps & {
  tab: string;
  instanceKey: string;
  config: InstanceConfig;
  saved: InstanceSettings | undefined;
  changed: string[];
  test: SsoTest;
}) {
  const inst = useInstance(instanceKey);
  switch (tab) {
    case "general":
      return <GeneralSettings node={inst?.node} config={config} {...props} />;
    case "sign-ups":
      return <SignUpSettings saved={saved} hasPasswordHere={inst?.me?.kind === AccountKind.LOCAL} {...props} />;
    case "sso":
      return <SsoSettings instanceKey={instanceKey} url={inst!.url} config={config} saved={saved} changed={changed} test={test} {...props} />;
    case "limits":
      return <LimitSettings {...props} />;
    case "calls":
      return <CallSettings config={config} {...props} />;
    case "moderation":
      return saved ? <ModerationSettings instanceKey={instanceKey} saved={saved} {...props} /> : null;
    case "federation":
      return saved ? <FederationSettings instanceKey={instanceKey} saved={saved} {...props} /> : null;
    case "gifs":
      return saved ? <GifSettings instanceKey={instanceKey} saved={saved} {...props} /> : null;
    case "privacy":
      return <PrivacySettings {...props} />;
    default:
      return null;
  }
}

/** The instance's name and addresses, and how it was started. */
function GeneralSettings({ node, config, draft, defaults, patch, resetter }: TabProps & { node: Parameters<typeof NewerRelease>[0]["node"]; config: InstanceConfig }) {
  const { t } = useI18n();
  const privateField = usePrivateField();
  return (
    <>
      <NewerRelease node={node} />
      <Setting id="name" title={t("instancesettings.nav.name")} hint={t("instancesettings.general.nameHint")} defaultLabel={defaults.name} {...resetter("name")}>
        <Input value={draft.name} maxLength={64} onChange={(e) => patch((d) => (d.name = e.target.value))} className="h-10 rounded-xl" />
      </Setting>
      <Setting
        id="public-url"
        title={t("instancesettings.nav.publicUrl")}
        hint={t("instancesettings.general.publicUrlHint")}
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
      <Setting id="web" title={t("instancesettings.nav.web")} delay={0.08} defaultLabel={onOff(t, defaults.web)} {...resetter("web")}>
        <Toggle
          checked={draft.web}
          disabled={!config.startup?.webBuiltIn}
          onChange={(web) => patch((d) => (d.web = web))}
          label={t("instancesettings.general.webLabel")}
          hint={t(config.startup?.webBuiltIn ? "instancesettings.general.webHint" : "instancesettings.general.webNotBuilt")}
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
  );
}

/** Who can make an account, and what accounts can do. */
function SignUpSettings({ draft, saved, defaults, patch, resetter, hasPasswordHere }: TabProps & { saved: InstanceSettings | undefined; hasPasswordHere: boolean }) {
  const lang = useI18n();
  const { t } = lang;
  return (
    <>
      <Setting
        id="local-accounts"
        title={t("instancesettings.nav.localAccounts")}
        hint={t("instancesettings.signUps.localHint")}
        defaultLabel={labelOf(t, ACCOUNTS_LABEL, defaults.localAccounts)}
        {...resetter("local_accounts")}
      >
        <Choice
          value={draft.localAccounts}
          onChange={(v) => patch((d) => (d.localAccounts = v))}
          options={[
            { value: LocalAccounts.OPEN, label: t("instancesettings.signUps.open"), hint: t("instancesettings.signUps.localOpenHint"), icon: <DoorOpenIcon className="size-4" /> },
            { value: LocalAccounts.CLOSED, label: t("instancesettings.signUps.closed"), hint: t("instancesettings.signUps.localClosedHint"), icon: <DoorClosedIcon className="size-4" /> },
            {
              value: LocalAccounts.OFF,
              label: t("serversettings.shared.off"),
              hint: t("instancesettings.signUps.localOffHint"),
              icon: <LockIcon className="size-4" />,
              disabled:
                draft.localAccounts === LocalAccounts.OFF || linkedWorks(draft) || ssoWorks(draft)
                  ? undefined
                  : t("instancesettings.signUps.localOffNeeds"),
            },
          ]}
        />
        <Notice show={draft.localAccounts === LocalAccounts.OFF && saved?.localAccounts !== LocalAccounts.OFF && hasPasswordHere}>
          {t("instancesettings.signUps.localOffNotice")}
        </Notice>
      </Setting>
      <Setting
        id="linked-accounts"
        title={t("instancesettings.nav.linkedAccounts")}
        hint={t("instancesettings.signUps.linkedHint")}
        defaultLabel={labelOf(t, ACCOUNTS_LABEL, defaults.linkedAccounts)}
        delay={0.04}
        {...resetter("linked_accounts")}
      >
        <Choice
          value={draft.linkedAccounts}
          onChange={(v) => patch((d) => (d.linkedAccounts = v))}
          options={[
            { value: LinkedAccounts.OPEN, label: t("instancesettings.signUps.open"), hint: t("instancesettings.signUps.linkedOpenHint"), icon: <Flower2Icon className="size-4" /> },
            { value: LinkedAccounts.CLOSED, label: t("instancesettings.signUps.closed"), hint: t("instancesettings.signUps.linkedClosedHint"), icon: <DoorClosedIcon className="size-4" /> },
            {
              value: LinkedAccounts.OFF,
              label: t("serversettings.shared.off"),
              hint: t("instancesettings.signUps.linkedOffHint"),
              icon: <LockIcon className="size-4" />,
              disabled:
                draft.linkedAccounts === LinkedAccounts.OFF || draft.localAccounts !== LocalAccounts.OFF || ssoWorks(draft)
                  ? undefined
                  : t("instancesettings.signUps.linkedOffNeeds"),
            },
          ]}
        />
        <Notice show={draft.linkedAccounts !== LinkedAccounts.OFF && !canReturnTo(draft.publicUrl)}>
          {t("instancesettings.signUps.linkedNotice")}
        </Notice>
      </Setting>
      <Setting
        id="linked-issuer"
        title={t("instancesettings.nav.linkedIssuer")}
        hint={t("instancesettings.signUps.issuerHint")}
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
        title={t("instancesettings.nav.serverCreation")}
        defaultLabel={labelOf(t, CREATION_LABEL, defaults.serverCreation)}
        delay={0.12}
        {...resetter("server_creation")}
      >
        <Choice
          value={draft.serverCreation}
          onChange={(v) => patch((d) => (d.serverCreation = v))}
          options={[
            { value: ServerCreation.EVERYONE, label: t("instancesettings.signUps.everyone"), hint: t("instancesettings.signUps.serverEveryoneHint"), icon: <UsersIcon className="size-4" /> },
            { value: ServerCreation.ADMINS, label: t("instancesettings.signUps.admins"), hint: t("instancesettings.signUps.adminsHint"), icon: <CrownIcon className="size-4" /> },
            { value: ServerCreation.DISABLED, label: t("instancesettings.signUps.nobody"), hint: t("instancesettings.signUps.serverNobodyHint"), icon: <BanIcon className="size-4" /> },
          ]}
        />
      </Setting>
      <Setting
        id="servers-per-account"
        title={t("instancesettings.nav.serversPerAccount")}
        hint={t("instancesettings.signUps.serversPerAccountHint")}
        defaultLabel={count(lang, defaults.serversPerAccount)}
        delay={0.16}
        {...resetter("servers_per_account")}
      >
        <Cap label={t("instancesettings.shared.upTo")} value={draft.serversPerAccount} onChange={(v) => patch((d) => (d.serversPerAccount = v))} />
      </Setting>
      <Setting
        id="agent-creation"
        title={t("instancesettings.nav.agentCreation")}
        hint={t("instancesettings.signUps.agentHint")}
        defaultLabel={labelOf(t, AGENT_CREATION_LABEL, defaults.agentCreation)}
        delay={0.2}
        {...resetter("agent_creation")}
      >
        <Choice
          value={draft.agentCreation}
          onChange={(v) => patch((d) => (d.agentCreation = v))}
          options={[
            { value: AgentCreation.EVERYONE, label: t("instancesettings.signUps.everyone"), hint: t("instancesettings.signUps.agentEveryoneHint"), icon: <BotIcon className="size-4" /> },
            { value: AgentCreation.ADMINS, label: t("instancesettings.signUps.admins"), hint: t("instancesettings.signUps.adminsHint"), icon: <CrownIcon className="size-4" /> },
            { value: AgentCreation.DISABLED, label: t("instancesettings.signUps.nobody"), hint: t("instancesettings.signUps.agentNobodyHint"), icon: <BanIcon className="size-4" /> },
          ]}
        />
      </Setting>
      <Setting
        id="mcp"
        title={t("instancesettings.nav.mcp")}
        defaultLabel={onOff(t, defaults.mcp)}
        delay={0.22}
        {...resetter("mcp")}
      >
        <Toggle
          checked={draft.mcp}
          onChange={(on) => patch((d) => (d.mcp = on))}
          label={t("instancesettings.signUps.mcpLabel")}
          hint={t("instancesettings.signUps.mcpHint")}
        />
      </Setting>
      <Setting
        id="shared-channels"
        title={t("serversettings.nav.shared")}
        defaultLabel={onOff(t, defaults.sharedChannels)}
        delay={0.24}
        {...resetter("shared_channels")}
      >
        <Toggle
          checked={draft.sharedChannels}
          onChange={(on) => patch((d) => (d.sharedChannels = on))}
          label={t("instancesettings.signUps.sharedLabel")}
          hint={t("instancesettings.signUps.sharedHint")}
        />
      </Setting>
      <Setting
        id="profile-effects"
        title={t("instancesettings.nav.profileEffects")}
        defaultLabel={onOff(t, defaults.profileEffects)}
        delay={0.28}
        {...resetter("profile_effects")}
      >
        <Toggle
          checked={draft.profileEffects}
          onChange={(on) => patch((d) => (d.profileEffects = on))}
          label={t("instancesettings.signUps.effectsLabel")}
          hint={t("instancesettings.signUps.effectsHint")}
        />
      </Setting>
      <Setting
        id="rich-presence"
        title={t("instancesettings.nav.richPresence")}
        defaultLabel={onOff(t, defaults.richPresence)}
        delay={0.32}
        {...resetter("rich_presence")}
      >
        <Toggle
          checked={draft.richPresence}
          onChange={(on) => patch((d) => (d.richPresence = on))}
          label={t("instancesettings.signUps.presenceLabel")}
          hint={t("instancesettings.signUps.presenceHint")}
        />
      </Setting>
    </>
  );
}

/** Signing in through the provider to try it, from `useAction`. */
type SsoTest = { go: (...args: Parameters<typeof startSsoSignIn>) => Promise<unknown>; pending: boolean; error: string | null };

/** Single sign-on for the whole instance: whether it makes accounts, and the identity provider. */
function SsoSettings({
  instanceKey,
  url,
  config,
  draft,
  saved,
  defaults,
  changed,
  patch,
  resetter,
  test,
}: TabProps & { instanceKey: string; url: string; config: InstanceConfig; saved: InstanceSettings | undefined; changed: string[]; test: SsoTest }) {
  const { t } = useI18n();
  const patchProvider = (fn: (p: IdentityProvider) => void) =>
    patch((d) => {
      d.ssoProvider = fullProvider(d.ssoProvider);
      fn(d.ssoProvider);
    });
  return (
    <>
      <Setting
        id="sso-accounts"
        title={t("instancesettings.nav.ssoAccounts")}
        hint={t("instancesettings.sso.accountsHint")}
        defaultLabel={labelOf(t, ACCOUNTS_LABEL, defaults.ssoAccounts)}
        {...resetter("sso_accounts")}
      >
        <Choice
          value={draft.ssoAccounts === SsoAccounts.UNSPECIFIED ? SsoAccounts.OFF : draft.ssoAccounts}
          onChange={(v) => patch((d) => (d.ssoAccounts = v))}
          options={[
            {
              value: SsoAccounts.OPEN,
              label: t("instancesettings.signUps.open"),
              hint: t("instancesettings.sso.openHint"),
              icon: <BuildingIcon className="size-4" />,
              disabled: providerReady(draft.ssoProvider) ? undefined : t("instancesettings.sso.setUpFirst"),
            },
            {
              value: SsoAccounts.CLOSED,
              label: t("instancesettings.signUps.closed"),
              hint: t("instancesettings.sso.closedHint"),
              icon: <DoorClosedIcon className="size-4" />,
              disabled: providerReady(draft.ssoProvider) ? undefined : t("instancesettings.sso.setUpFirst"),
            },
            {
              value: SsoAccounts.OFF,
              label: t("serversettings.shared.off"),
              hint: t("instancesettings.sso.offHint"),
              icon: <LockIcon className="size-4" />,
              disabled:
                draft.ssoAccounts === SsoAccounts.OFF || draft.localAccounts !== LocalAccounts.OFF || linkedWorks(draft)
                  ? undefined
                  : t("instancesettings.sso.offNeeds"),
            },
          ]}
        />
        <Notice show={draft.ssoAccounts !== SsoAccounts.OFF && draft.ssoAccounts !== SsoAccounts.UNSPECIFIED && !canReturnTo(draft.publicUrl)}>
          {t("instancesettings.sso.notice")}
        </Notice>
      </Setting>
      <IdentityProviderForm
        value={fullProvider(draft.ssoProvider)}
        onChange={patchProvider}
        serviceProvider={config.ssoServiceProvider}
        offHint={t("instancesettings.sso.noProvider")}
        test={{
          onTest: () => void test.go(url, window.location.pathname, { key: instanceKey }),
          pending: test.pending,
          error: test.error,
          blocked: changed.includes("sso_provider")
            ? t("instancesettings.sso.saveFirst")
            : !providerReady(saved?.ssoProvider)
              ? t("instancesettings.sso.fillFirst")
              : undefined,
        }}
      />
    </>
  );
}

/** What servers get by default, and how much people can upload and do. */
function LimitSettings({ draft, defaults, patch, resetter }: TabProps) {
  const lang = useI18n();
  const { t } = lang;
  return (
    <>
      <Setting
        id="default-limits"
        title={t("instancesettings.nav.defaultLimits")}
        hint={t("instancesettings.limits.defaultHint")}
        defaultLabel={t("instancesettings.limits.defaults", {
          members: count(lang, defaults.defaultLimits?.members),
          channels: count(lang, defaults.defaultLimits?.channels),
          storage: size(lang, defaults.defaultLimits?.storageBytes),
          files: size(lang, defaults.defaultLimits?.attachmentBytes),
          emoji: count(lang, defaults.defaultLimits?.emojis),
          recordings: size(lang, defaults.defaultLimits?.recordingBytes),
        })}
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
          <Cap label={t("serversettings.nav.members")} value={draft.defaultLimits?.members} onChange={(v) => patch((d) => (d.defaultLimits!.members = v))} />
          <Cap label={t("serversettings.nav.channels")} value={draft.defaultLimits?.channels} onChange={(v) => patch((d) => (d.defaultLimits!.channels = v))} />
          <Cap label={t("serversettings.usage.storage")} bytes value={draft.defaultLimits?.storageBytes} onChange={(v) => patch((d) => (d.defaultLimits!.storageBytes = v))} />
          <Cap label={t("serversettings.limits.files")} bytes value={draft.defaultLimits?.attachmentBytes} onChange={(v) => patch((d) => (d.defaultLimits!.attachmentBytes = v))} />
          <Cap label={t("serversettings.nav.emoji")} value={draft.defaultLimits?.emojis} onChange={(v) => patch((d) => (d.defaultLimits!.emojis = v))} />
          <Cap label={t("serversettings.nav.recordings")} bytes value={draft.defaultLimits?.recordingBytes} onChange={(v) => patch((d) => (d.defaultLimits!.recordingBytes = v))} />
        </div>
      </Setting>
      <Setting
        id="picture-uploads"
        title={t("instancesettings.nav.pictureUploads")}
        hint={t("instancesettings.limits.pictureHint")}
        defaultLabel={size(lang, defaults.pictureUploadBytes)}
        delay={0.04}
        {...resetter("picture_upload_bytes")}
      >
        <Cap label={t("instancesettings.shared.upTo")} bytes value={draft.pictureUploadBytes} onChange={(v) => patch((d) => (d.pictureUploadBytes = v))} />
      </Setting>
      <Setting
        id="picture-uploads-per-day"
        title={t("instancesettings.limits.picturesPerDay")}
        hint={t("instancesettings.limits.picturesPerDayHint")}
        defaultLabel={size(lang, defaults.pictureUploadBytesPerDay)}
        delay={0.08}
        {...resetter("picture_upload_bytes_per_day")}
      >
        <Cap
          label={t("instancesettings.shared.upTo")}
          bytes
          value={draft.pictureUploadBytesPerDay}
          onChange={(v) => patch((d) => (d.pictureUploadBytesPerDay = v))}
        />
      </Setting>
      <Setting
        id="attachment-uploads"
        title={t("instancesettings.limits.attachment")}
        hint={t("instancesettings.limits.attachmentHint")}
        defaultLabel={size(lang, defaults.attachmentUploadBytes)}
        delay={0.12}
        {...resetter("attachment_upload_bytes")}
      >
        <Cap label={t("instancesettings.shared.upTo")} bytes value={draft.attachmentUploadBytes} onChange={(v) => patch((d) => (d.attachmentUploadBytes = v))} />
      </Setting>
      <Setting
        id="attachment-uploads-per-day"
        title={t("instancesettings.limits.attachmentPerDay")}
        hint={t("instancesettings.limits.attachmentPerDayHint")}
        defaultLabel={size(lang, defaults.attachmentUploadBytesPerDay)}
        delay={0.16}
        {...resetter("attachment_upload_bytes_per_day")}
      >
        <Cap
          label={t("instancesettings.shared.upTo")}
          bytes
          value={draft.attachmentUploadBytesPerDay}
          onChange={(v) => patch((d) => (d.attachmentUploadBytesPerDay = v))}
        />
      </Setting>
      <Setting
        id="voice-message-seconds"
        title={t("instancesettings.limits.voiceSeconds")}
        hint={t("instancesettings.limits.voiceSecondsHint")}
        defaultLabel={
          defaults.voiceMessageSeconds === undefined
            ? t("instancesettings.shared.noLimit")
            : t("instancesettings.limits.seconds", { count: Number(defaults.voiceMessageSeconds) })
        }
        delay={0.2}
        {...resetter("voice_message_seconds")}
      >
        <Cap label={t("instancesettings.limits.secondsLabel")} value={draft.voiceMessageSeconds} onChange={(v) => patch((d) => (d.voiceMessageSeconds = v))} />
      </Setting>
      <Setting
        id="voice-message-bytes"
        title={t("instancesettings.limits.voiceBytes")}
        hint={t("instancesettings.limits.voiceBytesHint")}
        defaultLabel={size(lang, defaults.voiceMessageBytes)}
        delay={0.24}
        {...resetter("voice_message_bytes")}
      >
        <Cap label={t("instancesettings.shared.upTo")} bytes value={draft.voiceMessageBytes} onChange={(v) => patch((d) => (d.voiceMessageBytes = v))} />
      </Setting>
      <Setting
        id="voice-message-bytes-per-day"
        title={t("instancesettings.limits.voicePerDay")}
        hint={t("instancesettings.limits.voicePerDayHint")}
        defaultLabel={size(lang, defaults.voiceMessageBytesPerDay)}
        delay={0.28}
        {...resetter("voice_message_bytes_per_day")}
      >
        <Cap
          label={t("instancesettings.shared.upTo")}
          bytes
          value={draft.voiceMessageBytesPerDay}
          onChange={(v) => patch((d) => (d.voiceMessageBytesPerDay = v))}
        />
      </Setting>
      <Setting
        id="poll-votes-per-minute"
        title={t("instancesettings.limits.pollVotes")}
        hint={t("instancesettings.limits.pollVotesHint")}
        defaultLabel={perMinute(t, defaults.pollVotesPerMinute)}
        delay={0.2}
        {...resetter("poll_votes_per_minute")}
      >
        <Cap label={t("instancesettings.shared.upTo")} placeholder="30" value={draft.pollVotesPerMinute} onChange={(v) => patch((d) => (d.pollVotesPerMinute = v))} />
      </Setting>
      <Setting
        id="commands-per-minute"
        title={t("instancesettings.limits.commands")}
        hint={t("instancesettings.limits.commandsHint")}
        defaultLabel={perMinute(t, defaults.commandsPerMinute)}
        delay={0.22}
        {...resetter("commands_per_minute")}
      >
        <Cap label={t("instancesettings.shared.upTo")} placeholder="20" value={draft.commandsPerMinute} onChange={(v) => patch((d) => (d.commandsPerMinute = v))} />
      </Setting>
    </>
  );
}

/** The anonymous usage signal, and what it sends. */
function PrivacySettings({ draft, defaults, patch, resetter }: TabProps) {
  const { t } = useI18n();
  return (
    <>
      <Setting id="telemetry" title={t("instancesettings.nav.telemetry")} defaultLabel={onOff(t, defaults.telemetry)} {...resetter("telemetry")}>
        <Toggle
          checked={draft.telemetry}
          onChange={(telemetry) => patch((d) => (d.telemetry = telemetry))}
          label={t("instancesettings.privacy.label")}
          hint={t("instancesettings.privacy.hint")}
        />
        <ul className="grid gap-1.5 text-xs text-muted-foreground sm:grid-cols-2">
          {TELEMETRY_LINES.map(
            (line, n) => (
              <motion.li
                key={line}
                initial={{ opacity: 0, x: -8 }}
                animate={{ opacity: 1, x: 0 }}
                transition={{ ...SPRING, delay: 0.1 + n * 0.05 }}
                className="flex items-center gap-2"
              >
                <ShieldCheckIcon className="size-3.5 shrink-0 text-primary" /> {t(line)}
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
          {t("instancesettings.privacy.fields")}
        </a>
      </Setting>
    </>
  );
}

/** What the telemetry sends, as catalog keys. */
const TELEMETRY_LINES: readonly Key[] = [
  "instancesettings.privacy.counts",
  "instancesettings.privacy.storage",
  "instancesettings.privacy.options",
  "instancesettings.privacy.version",
  "instancesettings.privacy.errors",
];

const ACCOUNTS_LABEL: Record<number, Key> = {
  [LocalAccounts.OPEN]: "instancesettings.shared.open",
  [LocalAccounts.CLOSED]: "instancesettings.shared.closed",
  [LocalAccounts.OFF]: "instancesettings.shared.off",
};
/** A choice's default in words, from one of the tables here; a value without a word shows nothing, as before. */
const labelOf = (t: I18n["t"], table: Record<number, Key>, value: number) => (table[value] ? t(table[value]) : undefined);

/** "30 a minute", or "no limit". */
const perMinute = (t: I18n["t"], n: bigint | undefined) =>
  n === undefined ? t("instancesettings.shared.noLimit") : t("instancesettings.shared.perMinute", { count: Number(n) });

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

const AGENT_CREATION_LABEL: Record<number, Key> = {
  [AgentCreation.EVERYONE]: "instancesettings.shared.everyone",
  [AgentCreation.ADMINS]: "instancesettings.shared.admins",
  [AgentCreation.DISABLED]: "instancesettings.shared.nobody",
};

const CREATION_LABEL: Record<number, Key> = {
  [ServerCreation.EVERYONE]: "instancesettings.shared.everyone",
  [ServerCreation.ADMINS]: "instancesettings.shared.admins",
  [ServerCreation.DISABLED]: "instancesettings.shared.nobody",
};

/** Settings copied by a function of their own, beside the switch below. */
const COPIED = [
  ...CALL_FIELDS,
  { path: "shared_channels", copy: (into: InstanceSettings, from: InstanceSettings) => (into.sharedChannels = from.sharedChannels) },
  { path: "mcp", copy: (into: InstanceSettings, from: InstanceSettings) => (into.mcp = from.mcp) },
  { path: "profile_effects", copy: (into: InstanceSettings, from: InstanceSettings) => (into.profileEffects = from.profileEffects) },
  { path: "rich_presence", copy: (into: InstanceSettings, from: InstanceSettings) => (into.richPresence = from.richPresence) },
];

/** Copies the named settings from one draft into another. */
function mergeFields(into: InstanceSettings, from: InstanceSettings, paths: string[]) {
  for (const path of paths) {
    COPIED.find((f) => f.path === path)?.copy(into, from);
    MODERATION_FIELDS.find((f) => f.path === path)?.copy(into, from);
    FEDERATION_FIELDS.find((f) => f.path === path)?.copy(into, from);
    GIF_FIELDS.find((f) => f.path === path)?.copy(into, from);
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
      case "attachment_upload_bytes":
        into.attachmentUploadBytes = from.attachmentUploadBytes;
        break;
      case "attachment_upload_bytes_per_day":
        into.attachmentUploadBytesPerDay = from.attachmentUploadBytesPerDay;
        break;
      case "voice_message_seconds":
        into.voiceMessageSeconds = from.voiceMessageSeconds;
        break;
      case "voice_message_bytes":
        into.voiceMessageBytes = from.voiceMessageBytes;
        break;
      case "voice_message_bytes_per_day":
        into.voiceMessageBytesPerDay = from.voiceMessageBytesPerDay;
        break;
      case "poll_votes_per_minute":
        into.pollVotesPerMinute = from.pollVotesPerMinute;
        break;
      case "commands_per_minute":
        into.commandsPerMinute = from.commandsPerMinute;
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
  const { t } = useI18n();
  const any = draft.allowedOrigins.includes("*");
  const privateField = usePrivateField();
  const [list, setList] = useState(any ? "" : draft.allowedOrigins.join("\n"));
  // The text keeps what you typed (blank lines and all) until the list changes some
  // other way, such as Discard or Reset; then it shows the list.
  const origins = any ? null : draft.allowedOrigins.join("\n");
  const [synced, setSynced] = useState(origins);
  if (origins !== synced) {
    setSynced(origins);
    if (origins !== null && list.split("\n").map((o) => o.trim()).filter(Boolean).join("\n") !== origins) setList(origins);
  }
  return (
    <Setting
      id="origins"
      title={t("instancesettings.nav.origins")}
      hint={t("instancesettings.origins.hint")}
      defaultLabel={defaultLabel === "*" ? t("instancesettings.origins.anySiteDefault") : defaultLabel}
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
          { value: "any", label: t("instancesettings.origins.any"), hint: t("instancesettings.origins.anyHint"), icon: <GlobeIcon className="size-4" /> },
          { value: "list", label: t("instancesettings.origins.list"), hint: t("instancesettings.origins.listHint"), icon: <LockIcon className="size-4" /> },
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
        <p className="mt-1.5 text-xs text-muted-foreground">
          <T k="instancesettings.origins.note" values={{ page: <Private text={window.location.origin} /> }} />
        </p>
      </motion.div>
    </Setting>
  );
}

/** How the process was started: read-only, set by whoever runs the instance. */
function Startup({ config }: { config: InstanceConfig }) {
  const { t } = useI18n();
  const s = config.startup;
  if (!s) return null;
  const facts: { label: string; value: ReactNode; good?: boolean }[] = [
    { label: t("instancesettings.startup.version"), value: s.version },
    { label: t("instancesettings.startup.port"), value: s.port },
    { label: t("instancesettings.startup.encryption"), value: onOff(t, s.encryption), good: s.encryption },
    { label: t("instancesettings.startup.adminToken"), value: t(s.adminToken ? "instancesettings.shared.set" : "instancesettings.startup.notSet") },
    { label: t("instancesettings.startup.hosting"), value: s.hosted ? "Waifu Devs" : t("instancesettings.startup.selfHosted") },
  ];
  return (
    <motion.section
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ ...SPRING, delay: 0.16 }}
      className="mt-2 rounded-2xl border border-dashed p-4"
    >
      <h3 className="text-sm font-extrabold">{t("instancesettings.startup.title")}</h3>
      <p className="mt-0.5 text-xs text-muted-foreground">{t("instancesettings.startup.hint")}</p>
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

