import { clone } from "@bufbuild/protobuf";
import { ShieldAlertIcon } from "lucide-react";
import { AutoModProviderSettingsSchema, type AutoModProviderSettings, type InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import type { I18n } from "@/i18n/react";

const fingerprint = (p: AutoModProviderSettings) => [
  p.id,
  p.enabled,
  p.apiKey.trim(),
  p.model.trim(),
  p.accountId.trim().toLowerCase(),
  p.name.trim(),
  p.url.trim(),
  p.header.trim().toLowerCase(),
];

/** The instance settings the Moderation page reads. */
export const MODERATION_FIELDS: { path: string; get: (s: InstanceSettings) => unknown; copy: (into: InstanceSettings, from: InstanceSettings) => void }[] = [
  {
    path: "automod_providers",
    get: (s) => JSON.stringify(s.automodProviders.map(fingerprint)),
    copy: (into, from) => (into.automodProviders = from.automodProviders.map((p) => clone(AutoModProviderSettingsSchema, p))),
  },
  {
    path: "automod_checks_per_day",
    get: (s) => s.automodChecksPerDay,
    copy: (into, from) => (into.automodChecksPerDay = from.automodChecksPerDay),
  },
];

/** The Moderation page in the settings menu, in the app's language. */
export const moderationSection = (t: I18n["t"]) => ({
  id: "moderation",
  label: t("instancesettings.nav.moderation"),
  icon: ShieldAlertIcon,
  description: t("instancesettings.nav.moderationAbout"),
  keywords: "automod ai jev typesafe cloudflare clef workers smart filter custom webhook own",
  settings: [
    { id: "automod-typesafe-jev", label: "TypeSafe Jev", keywords: "automod ai moderation key" },
    { id: "automod-cloudflare-clef", label: "Cloudflare Clef", keywords: "automod ai moderation workers token account" },
    { id: "automod-custom", label: t("instancesettings.nav.automodCustom"), keywords: "automod custom webhook classifier own endpoint" },
    { id: "automod-checks-per-day", label: t("instancesettings.nav.automodChecks"), keywords: "automod limit cap budget cost quota daily" },
  ],
});
