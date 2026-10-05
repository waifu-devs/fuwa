import { NetworkIcon } from "lucide-react";
import type { InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import type { I18n } from "@/i18n/react";

/** The instance settings the Federation page reads. */
export const FEDERATION_FIELDS: { path: string; get: (s: InstanceSettings) => unknown; copy: (into: InstanceSettings, from: InstanceSettings) => void }[] = [
  { path: "federation", get: (s) => s.federation, copy: (into, from) => (into.federation = from.federation) },
  {
    path: "federation_blocked_hosts",
    get: (s) => hosts(s.federationBlockedHosts).join("\n"),
    copy: (into, from) => (into.federationBlockedHosts = [...from.federationBlockedHosts]),
  },
  {
    path: "shared_remote_sends_per_minute",
    get: (s) => s.sharedRemoteSendsPerMinute,
    copy: (into, from) => (into.sharedRemoteSendsPerMinute = from.sharedRemoteSendsPerMinute),
  },
  { path: "shared_remote_people", get: (s) => s.sharedRemotePeople, copy: (into, from) => (into.sharedRemotePeople = from.sharedRemotePeople) },
  {
    path: "shared_remote_file_bytes_per_day",
    get: (s) => s.sharedRemoteFileBytesPerDay,
    copy: (into, from) => (into.sharedRemoteFileBytesPerDay = from.sharedRemoteFileBytesPerDay),
  },
  {
    path: "shared_file_fetches_in_flight",
    get: (s) => s.sharedFileFetchesInFlight,
    copy: (into, from) => (into.sharedFileFetchesInFlight = from.sharedFileFetchesInFlight),
  },
];

/** The Other instances page in the settings menu, in the app's language. */
export const federationSection = (t: I18n["t"]) => ({
  id: "federation",
  label: t("instancesettings.nav.federation"),
  icon: NetworkIcon,
  description: t("instancesettings.nav.federationAbout"),
  keywords: "federation federate instances share channels across key fingerprint block",
  settings: [
    { id: "federation", label: t("instancesettings.nav.federationOn"), keywords: "federation on off" },
    { id: "federation-identity", label: t("instancesettings.nav.federationIdentity"), keywords: "fingerprint key address rotate replace" },
    { id: "federation-check", label: t("instancesettings.nav.federationCheck"), keywords: "test reach ping" },
    { id: "federation-peers", label: t("instancesettings.nav.federationPeers"), keywords: "pinned peers" },
    { id: "federation-blocked", label: t("instancesettings.nav.federationBlocked"), keywords: "block list deny" },
    { id: "federation-sends", label: t("instancesettings.nav.federationSends"), keywords: "limit cap rate flood shared remote" },
    { id: "federation-people", label: t("instancesettings.nav.federationPeople"), keywords: "limit cap shared remote guests" },
    { id: "federation-files", label: t("instancesettings.nav.federationFiles"), keywords: "limit cap shared remote attachments bytes" },
    { id: "federation-fetches", label: t("instancesettings.nav.federationFetches"), keywords: "limit cap shared remote attachments busy" },
  ],
});

/** One host per line, trimmed, lowercased, each once. */
export const hosts = (list: string[]) => [...new Set(list.map((h) => h.trim().toLowerCase()).filter(Boolean))];
