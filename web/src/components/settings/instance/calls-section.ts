import { AudioLinesIcon } from "lucide-react";
import type { InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import type { I18n } from "@/i18n/react";

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
  { path: "camera_max_height", get: (s) => s.cameraMaxHeight, copy: (into, from) => (into.cameraMaxHeight = from.cameraMaxHeight) },
  { path: "camera_max_fps", get: (s) => s.cameraMaxFps, copy: (into, from) => (into.cameraMaxFps = from.cameraMaxFps) },
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
    { id: "camera-height", label: t("instancesettings.calls.cameraHeight"), keywords: "video resolution 1080p 720p ceiling quality" },
    { id: "camera-fps", label: t("instancesettings.calls.cameraFps"), keywords: "video frame rate fps ceiling quality" },
    { id: "ice-urls", label: t("instancesettings.nav.iceUrls"), keywords: "ice nat relay firewall" },
    { id: "turn-secret", label: t("instancesettings.nav.turnSecret"), keywords: "coturn relay password" },
  ],
});
