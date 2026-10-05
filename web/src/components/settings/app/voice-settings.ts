import type { I18n } from "@/i18n/react";

export const voiceSettings = (t: I18n["t"]) => [
  { id: "devices", label: t("appsettings.voice.devices"), keywords: "input output device headset" },
  { id: "mic-test", label: t("appsettings.voice.micTest"), keywords: "check level meter" },
  { id: "input-mode", label: t("appsettings.voice.inputMode"), keywords: "voice activity push to talk ptt" },
  { id: "sensitivity", label: t("appsettings.voice.sensitivity"), keywords: "threshold gate noise" },
  { id: "processing", label: t("appsettings.voice.processing"), keywords: "echo noise suppression gain" },
  { id: "camera", label: t("appsettings.voice.camera"), keywords: "video webcam mirror preview" },
  { id: "call-sounds", label: t("appsettings.voice.callSounds"), keywords: "ring ringtone join leave" },
];
