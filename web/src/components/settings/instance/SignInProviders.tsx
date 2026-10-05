import { clone } from "@bufbuild/protobuf";
import { DoorClosedIcon, DoorOpenIcon, ExternalLinkIcon, KeyRoundIcon, PowerOffIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { ProviderAccounts, SignInProviderSettingSchema, type InstanceSettings, type SignInProviderSetting } from "@/gen/fuwa/v1/admin_pb";
import { usePrivateField } from "@/components/Private";
import { ProviderMark } from "@/components/ProviderMarks";
import { Input } from "@/components/ui/input";
import { type I18n, useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";
import { Choice, Setting, SPRING, Toggle } from "../controls";
import { CopyRow } from "../IdentityProviderForm";
import { accountsOf, PROVIDERS, settingOf, type ProviderInfo } from "./provider-fields";

type Reset = { changed: boolean; onReset: () => void; resetting: boolean };

/**
 * Sign in with Google, X and Twitch: whether newcomers get accounts, and the
 * app each provider gave this instance. The instance talks to the provider
 * itself; apps never load anything from it.
 */
export function SignInProviderSettings({
  draft,
  saved,
  defaults,
  patch,
  resetter,
}: {
  draft: InstanceSettings;
  saved: InstanceSettings;
  defaults: InstanceSettings;
  patch: (fn: (d: InstanceSettings) => void) => void;
  resetter: (...paths: string[]) => Reset;
}) {
  const { t } = useI18n();
  const accounts = accountsOf(draft);
  const edit = (id: string, fn: (p: SignInProviderSetting) => void) =>
    patch((d) => {
      const next = PROVIDERS.map((p) => clone(SignInProviderSettingSchema, settingOf(d, p.id)));
      fn(next.find((p) => p.id === id)!);
      d.signInProviders = next;
    });
  const base = saved.publicUrl.trim().replace(/\/+$/, "");

  return (
    <>
      <Setting
        id="provider-accounts"
        title={t("instancesettings.nav.providerAccounts")}
        hint={t("instancesettings.providers.accountsHint")}
        defaultLabel={accountsLabel(t, accountsOf(defaults))}
        {...resetter("provider_accounts")}
      >
        <Choice
          value={accounts}
          onChange={(v) => patch((d) => (d.providerAccounts = v))}
          options={[
            {
              value: ProviderAccounts.OPEN,
              label: accountsLabel(t, ProviderAccounts.OPEN),
              hint: t("instancesettings.providers.openHint"),
              icon: <DoorOpenIcon className="size-4" />,
            },
            {
              value: ProviderAccounts.CLOSED,
              label: accountsLabel(t, ProviderAccounts.CLOSED),
              hint: t("instancesettings.providers.closedHint"),
              icon: <DoorClosedIcon className="size-4" />,
            },
            {
              value: ProviderAccounts.OFF,
              label: accountsLabel(t, ProviderAccounts.OFF),
              hint: t("instancesettings.providers.offHint"),
              icon: <PowerOffIcon className="size-4" />,
            },
          ]}
        />
      </Setting>
      {PROVIDERS.map((p, n) => (
        <ProviderRow
          key={p.id}
          provider={p}
          setting={settingOf(draft, p.id)}
          was={settingOf(saved, p.id)}
          redirect={base ? `${base}/sso/instance/providers/${p.id}` : ""}
          delay={0.04 * (n + 1)}
          off={accounts === ProviderAccounts.OFF}
          onEdit={(fn) => edit(p.id, fn)}
          reset={resetter("sign_in_providers")}
        />
      ))}
    </>
  );
}

function ProviderRow({
  provider,
  setting,
  was,
  redirect,
  delay,
  off,
  onEdit,
  reset,
}: {
  provider: ProviderInfo;
  setting: SignInProviderSetting;
  was: SignInProviderSetting;
  redirect: string;
  delay: number;
  off: boolean;
  onEdit: (fn: (p: SignInProviderSetting) => void) => void;
  reset: Reset;
}) {
  const { t } = useI18n();
  const { name } = provider;
  return (
    <Setting
      id={`provider-${provider.id}`}
      title={name}
      hint={t("instancesettings.providers.providerHint", { name })}
      delay={delay}
      defaultLabel={t("serversettings.shared.off")}
      {...reset}
    >
      <div className={cn("flex items-center gap-3", off && "opacity-60")}>
        <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-foreground text-primary">
          <ProviderMark id={provider.id} className="size-[18px]" />
        </span>
        <div className="flex-1">
          <Toggle
            checked={setting.enabled}
            onChange={(enabled) => onEdit((x) => (x.enabled = enabled))}
            label={t("instancesettings.providers.enable", { name })}
            hint={off ? t("instancesettings.providers.allOff") : undefined}
          />
        </div>
      </div>
      <AnimatePresence initial={false}>
        {setting.enabled && <AppFields key="app" provider={provider} setting={setting} was={was} redirect={redirect} onEdit={onEdit} />}
      </AnimatePresence>
    </Setting>
  );
}

/** The app the admin made at the provider: where it sends people back, and its client ID and secret. */
function AppFields({
  provider,
  setting,
  was,
  redirect,
  onEdit,
}: {
  provider: ProviderInfo;
  setting: SignInProviderSetting;
  was: SignInProviderSetting;
  redirect: string;
  onEdit: (fn: (p: SignInProviderSetting) => void) => void;
}) {
  const { t } = useI18n();
  const { name } = provider;
  const keepsSecret = was.clientSecretSet && setting.clientId.trim() === was.clientId.trim();
  const idMissing = !setting.clientId.trim();
  const secretMissing = !setting.clientSecret.trim() && !keepsSecret;
  return (
    <motion.div
      initial={{ opacity: 0, y: -6 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -6 }}
      transition={SPRING}
      className="flex flex-col gap-3"
    >
      <p className="text-sm text-muted-foreground">
        {t("instancesettings.providers.makeApp", { name })}{" "}
        <a href={provider.console} target="_blank" rel="noreferrer noopener" className="inline-flex items-center gap-1 font-bold text-primary hover:underline">
          {new URL(provider.console).host}
          <ExternalLinkIcon className="size-3" />
        </a>
      </p>
      {provider.id === "x" && <p className="rounded-xl bg-muted/60 px-3 py-2 text-xs text-muted-foreground">{t("instancesettings.providers.xScopes")}</p>}
      {provider.id === "twitch" && (
        <p className="rounded-xl bg-muted/60 px-3 py-2 text-xs text-muted-foreground">{t("instancesettings.providers.twitchState")}</p>
      )}
      {redirect ? (
        <CopyRow label={t("instancesettings.providers.redirect")} value={redirect} delay={0.05} />
      ) : (
        <p className="text-xs text-amber-600 dark:text-amber-400">{t("instancesettings.providers.needsAddress")}</p>
      )}
      <label className="flex flex-col gap-1.5">
        <span className="text-[0.7rem] font-bold text-muted-foreground uppercase">{t("instancesettings.providers.clientId")}</span>
        <Input
          value={setting.clientId}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => onEdit((x) => (x.clientId = e.target.value))}
          className={cn("h-10 rounded-xl font-mono text-sm", idMissing && "ring-2 ring-amber-500/50")}
          data-testid={`provider-${provider.id}-client-id`}
        />
      </label>
      <SecretField provider={provider} value={setting.clientSecret} saved={keepsSecret ? was.clientSecretHint || null : undefined} missing={secretMissing} onEdit={onEdit} />
      {(idMissing || secretMissing) && <p className="text-xs text-amber-600 dark:text-amber-400">{t("instancesettings.providers.missing")}</p>}
    </motion.div>
  );
}

/**
 * The client secret. `saved` is the kept secret's hint, null when it has
 * none, and undefined when no secret is kept for this client ID.
 */
function SecretField({
  provider,
  value,
  saved,
  missing,
  onEdit,
}: {
  provider: ProviderInfo;
  value: string;
  saved: string | null | undefined;
  missing: boolean;
  onEdit: (fn: (p: SignInProviderSetting) => void) => void;
}) {
  const { t } = useI18n();
  const privateField = usePrivateField();
  const placeholder =
    saved === undefined
      ? t("instancesettings.providers.pasteSecret", { name: provider.name })
      : saved
        ? t("instancesettings.shared.savedEnding", { hint: saved })
        : t("instancesettings.shared.saved");
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-[0.7rem] font-bold text-muted-foreground uppercase">{t("instancesettings.providers.clientSecret")}</span>
      <div className="relative">
        <KeyRoundIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          type="password"
          autoComplete="off"
          value={value}
          onChange={(e) => onEdit((x) => (x.clientSecret = e.target.value))}
          placeholder={placeholder}
          className={cn("h-10 rounded-xl pl-9 font-mono text-sm", privateField, missing && "ring-2 ring-amber-500/50")}
          data-testid={`provider-${provider.id}-client-secret`}
        />
      </div>
    </label>
  );
}

function accountsLabel(t: I18n["t"], v: ProviderAccounts) {
  if (v === ProviderAccounts.CLOSED) return t("instancesettings.providers.closed");
  if (v === ProviderAccounts.OFF) return t("serversettings.shared.off");
  return t("instancesettings.providers.open");
}
