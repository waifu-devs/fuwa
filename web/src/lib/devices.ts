/**
 * A device, as its User-Agent describes it: "Firefox on macOS", and whether
 * it's a phone, a tablet or a computer, for its icon. Good enough to tell
 * your own devices apart, which is all the Devices page needs. Names read in
 * the app's language (common.device.*); browsers and systems keep their own.
 */

import type { I18n, Key } from "../i18n/i18n.ts";
import { type Lang, relative } from "./durations.ts";

export type DeviceKind = "phone" | "tablet" | "computer" | "tool";

/** `browser` is the browser's own name, or one of ours (OURS) to look up in the catalog. */
export type Device = { browser: string; os: string; kind: DeviceKind };

/** Programs that aren't browsers, named in the app's language. */
const OURS: Record<string, Key> = { "fuwa-desktop": "common.device.desktop", script: "common.device.script" };

const BROWSERS: [RegExp, string][] = [
  [/fuwa-desktop/i, "fuwa-desktop"],
  [/Edg(e|A|iOS)?\//, "Edge"],
  [/OPR\/|Opera/, "Opera"],
  [/SamsungBrowser/, "Samsung Internet"],
  [/Vivaldi/, "Vivaldi"],
  [/Firefox\/|FxiOS/, "Firefox"],
  [/CriOS|Chrome\/|Chromium/, "Chrome"],
  [/Safari\//, "Safari"],
  [/tonic|grpc|okhttp|curl|python|Go-http/i, "script"],
];

const SYSTEMS: [RegExp, string][] = [
  [/iPhone/, "iPhone"],
  [/iPad/, "iPad"],
  [/Android/, "Android"],
  [/CrOS/, "ChromeOS"],
  [/Windows/, "Windows"],
  [/Mac OS X|Macintosh/, "macOS"],
  [/Linux/, "Linux"],
];

export function describeDevice(userAgent: string): Device {
  const browser = BROWSERS.find(([re]) => re.test(userAgent))?.[1] ?? "";
  const os = SYSTEMS.find(([re]) => re.test(userAgent))?.[1] ?? "";
  const kind: DeviceKind = /iPad|Tablet/.test(userAgent)
    ? "tablet"
    : /Mobi|iPhone|Android.+Mobile/.test(userAgent)
      ? "phone"
      : browser === "script"
        ? "tool"
        : "computer";
  return { browser, os, kind };
}

/** "Firefox on macOS", "Safari on iPhone", "An unknown device". */
export function deviceName(t: I18n["t"], d: Device) {
  const browser = Object.hasOwn(OURS, d.browser) ? t(OURS[d.browser]!) : d.browser;
  if (browser && d.os) return t("common.device.on", { browser, os: d.os });
  if (browser) return browser[0]!.toLocaleUpperCase() + browser.slice(1);
  if (d.os) return d.os;
  return t("common.device.unknown");
}

/** How long ago something was last used, as it reads after "Active" ("5 minutes ago", "yesterday"), or null for just now. */
export function activeWhen({ locale }: Lang, date: Date, now = Date.now()) {
  const minutes = Math.max(0, Math.round((now - date.getTime()) / 60_000));
  // Sessions note their use every few minutes.
  if (minutes < 6) return null;
  if (minutes < 60) return relative(locale, -minutes, "minute", "auto");
  const hours = Math.round(minutes / 60);
  if (hours < 24) return relative(locale, -hours, "hour", "auto");
  const days = Math.round(hours / 24);
  if (days < 30) return relative(locale, -days, "day", "auto");
  return relative(locale, -Math.round(days / 30), "month", "auto");
}

/** "Active now", "Active 5 minutes ago", "Active yesterday". */
export function activeAgo(lang: Lang, date: Date, now = Date.now()) {
  const when = activeWhen(lang, date, now);
  return when ? lang.t("common.device.active", { when }) : lang.t("common.device.activeNow");
}
