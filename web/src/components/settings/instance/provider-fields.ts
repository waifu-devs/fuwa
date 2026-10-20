import { clone, create } from "@bufbuild/protobuf";
import { LogInIcon } from "lucide-react";
import { EmailAccounts, ProviderAccounts, SignInProviderSettingSchema, type InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import type { I18n } from "@/i18n/react";

/** The providers fuwa knows, in the order it shows them, and where an admin makes an app for each. */
export const PROVIDERS = [
  { id: "google", name: "Google", console: "https://console.cloud.google.com/apis/credentials" },
  { id: "x", name: "X", console: "https://developer.x.com/en/portal/dashboard" },
  { id: "twitch", name: "Twitch", console: "https://dev.twitch.tv/console/apps" },
] as const;

export type ProviderInfo = (typeof PROVIDERS)[number];

/** A provider's settings, or a switched-off blank one when none are saved. */
export const settingOf = (s: InstanceSettings, id: string) =>
  s.signInProviders.find((p) => p.id === id) ?? create(SignInProviderSettingSchema, { id });

export const accountsOf = (s: InstanceSettings) => s.providerAccounts || ProviderAccounts.OPEN;

export const emailAccountsOf = (s: InstanceSettings) => s.emailAccounts || EmailAccounts.OPEN;

/** What each mail provider is called, by `InstanceSettings.mailProvider`. */
export const MAIL_PROVIDERS: Record<string, string> = { smtp: "SMTP", ses: "Amazon SES", resend: "Resend", cloudflare: "Cloudflare" };

/** The instance settings the sign-in providers page reads. */
export const SIGN_IN_PROVIDER_FIELDS: { path: string; get: (s: InstanceSettings) => unknown; copy: (into: InstanceSettings, from: InstanceSettings) => void }[] = [
  { path: "provider_accounts", get: accountsOf, copy: (into, from) => (into.providerAccounts = from.providerAccounts) },
  { path: "email_accounts", get: emailAccountsOf, copy: (into, from) => (into.emailAccounts = from.emailAccounts) },
  {
    path: "sign_in_providers",
    get: (s) =>
      PROVIDERS.map(({ id }) => {
        const p = settingOf(s, id);
        // A secret is never sent back: an empty field keeps the saved one.
        return [p.enabled, p.clientId.trim(), p.clientSecret.trim()].join("|");
      }).join("\n"),
    copy: (into, from) => (into.signInProviders = from.signInProviders.map((p) => clone(SignInProviderSettingSchema, p))),
  },
];

/** The sign-in providers page in the settings menu, in the app's language. */
export const signInProviderSection = (t: I18n["t"]) => ({
  id: "sign-in-providers",
  label: t("instancesettings.nav.signInProviders"),
  icon: LogInIcon,
  description: t("instancesettings.nav.signInProvidersAbout"),
  keywords: "google x twitter twitch oauth social login sign in",
  settings: [
    { id: "provider-accounts", label: t("instancesettings.nav.providerAccounts"), keywords: "sign up new accounts" },
    ...PROVIDERS.map((p) => ({ id: `provider-${p.id}`, label: p.name, keywords: "client id secret app" })),
    { id: "email-accounts", label: t("instancesettings.nav.emailAccounts"), keywords: "email code mail smtp ses resend" },
  ],
});
