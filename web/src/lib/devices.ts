/**
 * A device, as its User-Agent describes it: "Firefox on macOS", and whether
 * it's a phone, a tablet or a computer, for its icon. Good enough to tell
 * your own devices apart, which is all the Devices page needs.
 */

export type DeviceKind = "phone" | "tablet" | "computer" | "tool";

export type Device = { browser: string; os: string; kind: DeviceKind };

const BROWSERS: [RegExp, string][] = [
  [/fuwa-desktop/i, "fuwa desktop"],
  [/Edg(e|A|iOS)?\//, "Edge"],
  [/OPR\/|Opera/, "Opera"],
  [/SamsungBrowser/, "Samsung Internet"],
  [/Vivaldi/, "Vivaldi"],
  [/Firefox\/|FxiOS/, "Firefox"],
  [/CriOS|Chrome\/|Chromium/, "Chrome"],
  [/Safari\//, "Safari"],
  [/tonic|grpc|okhttp|curl|python|Go-http/i, "an app or script"],
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
      : browser === "an app or script"
        ? "tool"
        : "computer";
  return { browser, os, kind };
}

/** "Firefox on macOS", "Safari on iPhone", "An unknown device". */
export function deviceName(d: Device) {
  if (d.browser && d.os) return `${d.browser} on ${d.os}`;
  if (d.browser) return d.browser[0]!.toUpperCase() + d.browser.slice(1);
  if (d.os) return d.os;
  return "An unknown device";
}

/** "Active now", "Active 5 minutes ago", "Active 3 days ago". */
export function activeAgo(date: Date, now = Date.now()) {
  const minutes = Math.max(0, Math.round((now - date.getTime()) / 60_000));
  // Sessions note their use every few minutes.
  if (minutes < 6) return "Active now";
  const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  if (minutes < 60) return `Active ${rtf.format(-minutes, "minute")}`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `Active ${rtf.format(-hours, "hour")}`;
  const days = Math.round(hours / 24);
  if (days < 30) return `Active ${rtf.format(-days, "day")}`;
  return `Active ${rtf.format(-Math.round(days / 30), "month")}`;
}
