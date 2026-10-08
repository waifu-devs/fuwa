import { KeyRoundIcon, RadioTowerIcon, ServerIcon } from "lucide-react";
import { m as motion } from "motion/react";
import type { InstanceConfig, InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import { usePrivateField } from "@/components/Private";
import { Segmented } from "@/components/settings/account/common";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useI18n } from "@/i18n/react";
import { CEILING_FRAME_RATES, CEILING_HEIGHTS } from "@/lib/camera-quality";
import { cn } from "@/lib/utils";
import { Cap, Setting, SPRING, Toggle } from "../controls";

type Reset = { changed: boolean; onReset: () => void; resetting: boolean };

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
  // A ceiling set by hand to something the buttons don't offer still reads right in the default.
  const height = (n: bigint | number | undefined) =>
    n ? t("appsettings.voice.heightValue", { height: Number(n) }) : t("instancesettings.calls.noCeiling");
  const fps = (n: bigint | number | undefined) => (n ? t("appsettings.voice.fpsValue", { fps: Number(n) }) : t("instancesettings.calls.noCeiling"));
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
        id="camera-height"
        title={t("instancesettings.calls.cameraHeight")}
        hint={t("instancesettings.calls.cameraHeightHint")}
        delay={0.035}
        defaultLabel={height(defaults.cameraMaxHeight)}
        {...resetter("camera_max_height")}
      >
        <Segmented
          label={t("instancesettings.calls.cameraHeight")}
          value={Number(draft.cameraMaxHeight ?? 0)}
          onChange={(n) => patch((d) => (d.cameraMaxHeight = n ? BigInt(n) : undefined))}
          options={CEILING_HEIGHTS.map((n) => ({ value: n, label: height(n) }))}
          className="flex w-full max-w-md"
        />
      </Setting>
      <Setting
        id="camera-fps"
        title={t("instancesettings.calls.cameraFps")}
        hint={t("instancesettings.calls.cameraFpsHint")}
        delay={0.038}
        defaultLabel={fps(defaults.cameraMaxFps)}
        {...resetter("camera_max_fps")}
      >
        <Segmented
          label={t("instancesettings.calls.cameraFps")}
          value={Number(draft.cameraMaxFps ?? 0)}
          onChange={(n) => patch((d) => (d.cameraMaxFps = n ? BigInt(n) : undefined))}
          options={CEILING_FRAME_RATES.map((n) => ({ value: n, label: fps(n) }))}
          className="flex w-full max-w-md"
        />
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
