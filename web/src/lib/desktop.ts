// The desktop app's downloads, as an instance hands them out (Node.desktop_app).
// Pure, so the choice of file is tested without a browser.

import type { DesktopDownload } from "@/gen/fuwa/v1/types_pb";

/** DesktopSystem and DesktopPackage's numbers, so this file needs no generated code at runtime. */
export const SYSTEM = { windows: 1, macos: 2, linux: 3 } as const;
export const PACKAGE = { setup: 1, dmg: 2, appimage: 3, deb: 4 } as const;

export type System = keyof typeof SYSTEM;

/** Which system this browser runs on, or null where the desktop app doesn't (phones, tablets, ChromeOS). */
export function systemOf(userAgent: string, touchPoints = 0): System | null {
  if (/Android|iPhone|iPad|iPod|CrOS/.test(userAgent)) return null;
  if (/Windows/.test(userAgent)) return "windows";
  if (/Mac OS X|Macintosh/.test(userAgent)) return touchPoints > 1 ? null : "macos"; // iPads say they're Macs.
  if (/Linux/.test(userAgent)) return "linux";
  return null;
}

/** Each system's downloads, the one to offer first at the front. */
const ORDER: Record<number, number> = { [PACKAGE.setup]: 0, [PACKAGE.dmg]: 0, [PACKAGE.appimage]: 0, [PACKAGE.deb]: 1 };

export function downloadsFor<D extends Pick<DesktopDownload, "system" | "package">>(downloads: readonly D[], system: System): D[] {
  return downloads.filter((d) => d.system === SYSTEM[system]).sort((a, b) => (ORDER[a.package] ?? 9) - (ORDER[b.package] ?? 9));
}

/** The download's address on the instance at `instanceUrl`, under its path like its calls are. */
export const downloadUrl = (instanceUrl: string, path: string) => new URL(path.replace(/^\/+/, ""), instanceUrl.endsWith("/") ? instanceUrl : `${instanceUrl}/`).href;
