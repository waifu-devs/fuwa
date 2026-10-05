import { AudioLinesIcon, KeyRoundIcon, RadioTowerIcon, ServerIcon } from "lucide-react";
import { motion } from "motion/react";
import type { InstanceConfig, InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import { usePrivateField } from "@/components/Private";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { type I18n, useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";
import { Cap, Setting, SPRING, Toggle } from "../controls";

type Reset = { changed: boolean; onReset: () => void; resetting: boolean };

/** The instance settings calls read. */
export const CALL_FIELDS: { path: string; get: (s: InstanceSettings) => unknown; copy: (into: InstanceSettings, from: InstanceSettings) => void }[] = [
  { path: "calls", get: (s) => s.calls, copy: (into, from) => (into.calls = from.calls) },
  { path: "call_recordings", get: (s) => s.callRecordings, copy: (into, from) => (into.callRecordings = from.callRecordings) },
  {
    path: "call_recording_video",
    get: (s) => s.callRecordingVideo,
    copy: (into, from) => (into.callRecordingVideo = from.callRecordingVideo),
  },
  {
    path: "call_recordings_keep_days",
    get: (s) => s.callRecordingsKeepDays,
    copy: (into, from) => (into.callRecordingsKeepDays = from.callRecordingsKeepDays),
  },
  {
    path: "ice_urls",
    get: (s) => s.iceUrls.map((u) => u.trim()).filter(Boolean).join("\n"),
    copy: (into, from) => (into.iceUrls = [...from.iceUrls]),
  },
  // The server never sends the secret back, so an empty field keeps the saved one.
  {
    path: "turn_secret",
    get: (s) => s.turnSecret.trim(),
    copy: (into, from) => {
      into.turnSecret = from.turnSecret;
      into.turnSecretSet = from.turnSecretSet;
      into.turnSecretHint = from.turnSecretHint;
    },
  },
];

/** The Calls page in the settings menu, in the app's language. */
export const callSection = (t: I18n["t"]) => ({
  id: "calls",
  label: t("instancesettings.nav.calls"),
  icon: AudioLinesIcon,
  description: t("instancesettings.nav.callsAbout"),
  keywords: "voice webrtc stun turn ice media",
  settings: [
    { id: "calls-on", label: t("instancesettings.nav.calls"), keywords: "voice enable" },
    { id: "call-recordings", label: t("instancesettings.nav.callRecordings"), keywords: "record recordings tracks podcast" },
    { id: "call-recordings-keep", label: t("instancesettings.nav.callRecordingsKeep"), keywords: "retention expire delete days old recordings" },
    { id: "ice-urls", label: t("instancesettings.nav.iceUrls"), keywords: "ice nat relay firewall" },
    { id: "turn-secret", label: t("instancesettings.nav.turnSecret"), keywords: "coturn relay password" },
  ],
});

/** Calls on an instance: whether they're on, and how apps get through firewalls to its media server. */
export function CallSettings({
  config,
  draft,
  defaults,
  patch,
  resetter,
}: {
  config: InstanceConfig;
  draft: InstanceSettings;
  defaults: InstanceSettings;
  patch: (fn: (d: InstanceSettings) => void) => void;
  resetter: (...paths: string[]) => Reset;
}) {
  const { t } = useI18n();
  const on = (value: boolean) => t(value ? "instancesettings.shared.on" : "instancesettings.shared.off");
  const privateField = usePrivateField();
  const startup = config.startup;
  const media = startup?.mediaPort
    ? startup.mediaAddresses.length
      ? t("instancesettings.calls.portOn", { port: String(startup.mediaPort), addresses: startup.mediaAddresses.join(", ") })
      : t("instancesettings.calls.port", { port: String(startup.mediaPort) })
    : startup?.mediaAddresses.length
      ? startup.mediaAddresses.join(", ")
      : null;
  return (
    <>
      <Setting id="calls-on" title={t("instancesettings.nav.calls")} defaultLabel={on(defaults.calls)} {...resetter("calls")}>
        <Toggle
          checked={draft.calls}
          onChange={(calls) => patch((d) => (d.calls = calls))}
          label={t("instancesettings.calls.label")}
          hint={t("instancesettings.calls.hint")}
        />
        <motion.div
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ ...SPRING, delay: 0.1 }}
          className={cn("flex items-start gap-2.5 rounded-2xl border border-dashed p-3 text-sm", !media && "border-amber-500/50")}
        >
          <ServerIcon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <span className="min-w-0">
            <b>{t("instancesettings.calls.mediaServer")} </b>
            {media ? (
              <span className={cn("break-words text-muted-foreground", privateField)}>{media}</span>
            ) : (
              <span className="text-amber-600 dark:text-amber-400">{t("instancesettings.calls.notRunning")}</span>
            )}
            <span className="mt-0.5 block text-xs text-muted-foreground">{t("instancesettings.calls.setAtStart")}</span>
          </span>
        </motion.div>
      </Setting>
      <Setting
        id="call-recordings"
        title={t("instancesettings.nav.callRecordings")}
        delay={0.02}
        defaultLabel={on(defaults.callRecordings)}
        {...resetter("call_recordings")}
      >
        <Toggle
          checked={draft.callRecordings}
          onChange={(on) => patch((d) => (d.callRecordings = on))}
          label={t("instancesettings.calls.recordLabel")}
          hint={t("instancesettings.calls.recordHint")}
        />
      </Setting>
      <Setting
        id="call-recording-video"
        title={t("instancesettings.calls.video")}
        delay={0.025}
        defaultLabel={on(defaults.callRecordingVideo)}
        {...resetter("call_recording_video")}
      >
        <Toggle
          checked={draft.callRecordingVideo}
          onChange={(on) => patch((d) => (d.callRecordingVideo = on))}
          label={t("instancesettings.calls.videoLabel")}
          hint={t("instancesettings.calls.videoHint")}
        />
      </Setting>
      <Setting
        id="call-recordings-keep"
        title={t("instancesettings.nav.callRecordingsKeep")}
        hint={t("instancesettings.calls.keepHint")}
        delay={0.03}
        defaultLabel={
          defaults.callRecordingsKeepDays === undefined
            ? t("instancesettings.calls.untilDeleted")
            : t("instancesettings.calls.days", { count: Number(defaults.callRecordingsKeepDays) })
        }
        {...resetter("call_recordings_keep_days")}
      >
        <Cap label={t("instancesettings.calls.daysLabel")} value={draft.callRecordingsKeepDays} onChange={(v) => patch((d) => (d.callRecordingsKeepDays = v))} />
      </Setting>
      <Setting
        id="ice-urls"
        title={t("instancesettings.nav.iceUrls")}
        hint={t("instancesettings.calls.iceHint")}
        delay={0.04}
        defaultLabel={defaults.iceUrls.join(", ") || t("instancesettings.shared.none")}
        {...resetter("ice_urls")}
      >
        <div className="relative">
          <RadioTowerIcon className="pointer-events-none absolute top-3 left-3 size-4 text-muted-foreground" />
          <Textarea
            value={draft.iceUrls.join("\n")}
            onChange={(e) => patch((d) => (d.iceUrls = e.target.value.split("\n")))}
            rows={3}
            spellCheck={false}
            placeholder={"stun:stun.example.com:3478\nturn:turn.example.com:3478?transport=udp"}
            className={cn("rounded-xl pl-9 font-mono text-sm", privateField)}
          />
        </div>
      </Setting>
      <Setting
        id="turn-secret"
        title={t("instancesettings.nav.turnSecret")}
        hint={t("instancesettings.calls.turnHint")}
        delay={0.08}
        defaultLabel={t(defaults.turnSecretSet ? "instancesettings.shared.set" : "instancesettings.shared.none")}
        {...resetter("turn_secret")}
      >
        <div className="relative">
          <KeyRoundIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="password"
            autoComplete="off"
            value={draft.turnSecret}
            onChange={(e) => patch((d) => (d.turnSecret = e.target.value))}
            placeholder={
              draft.turnSecretSet
                ? draft.turnSecretHint
                  ? t("instancesettings.shared.savedEnding", { hint: draft.turnSecretHint })
                  : t("instancesettings.shared.saved")
                : t("instancesettings.calls.noSecret")
            }
            className="h-10 rounded-xl pl-9"
          />
        </div>
      </Setting>
    </>
  );
}
