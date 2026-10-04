import { BellIcon, BellOffIcon, BellRingIcon, FingerprintIcon, SlidersHorizontalIcon } from "lucide-react";
import { NotificationLevel } from "@/gen/fuwa/v1/types_pb";
import { run, updateNotifications, type NotificationPatch } from "@/fuwa/actions";
import { getInstance } from "@/fuwa/hooks";
import { notificationKey } from "@/fuwa/store";
import { attempt } from "@/components/menus/MenuDialogs";
import type { MenuAction, MenuEntry } from "@/lib/context-menu";
import { isMuted, LEVELS, MUTE_FOR, mutedLabel } from "@/lib/notifications";
import { getPrefs } from "@/lib/prefs";
import { copy, openSettings } from "@/lib/ui";

type Navigate = (options: { to: string; params: Record<string, string> }) => void;
let navigate: Navigate | null = null;

/** The menu host hands over the router's navigate; menus can't import the router (it imports them). */
export const setMenuNavigate = (fn: Navigate | null) => {
  navigate = fn;
};

/** Goes somewhere in the app. */
export const goTo: Navigate = (options) => navigate?.(options);

/** "Copy … ID", only with Developer Mode on, as everywhere else. */
export function copyIdItem(id: string, what: string): MenuAction | null {
  if (!getPrefs().developerMode || !id) return null;
  return { id: "copy-id", label: `Copy ${what} ID`, icon: FingerprintIcon, onSelect: () => copy(id, `${what} ID`) };
}

/**
 * A link to a place in a server, on the instance's own address as invite
 * links are, so anyone with access can open it. Copying never shows it: in
 * streamer mode the toast still only says what was copied.
 */
export function placeLink(instanceKey: string, ...path: string[]): string {
  const inst = getInstance(instanceKey);
  const base = (inst?.node?.publicUrl || inst?.url || location.origin).replace(/\/+$/, "");
  return [base, ...[instanceKey, ...path].map(encodeURIComponent)].join("/");
}

/**
 * Mute or unmute a server or channel for a while, as the bell and the server
 * menu do, and (for channels) what it notifies about.
 */
export function notificationEntries(instanceKey: string, serverId: string, channelId: string, what: "server" | "channel"): MenuEntry[] {
  const inst = getInstance(instanceKey);
  const settings = inst?.notifications[notificationKey(serverId, channelId)];
  const change = (patch: NotificationPatch) => attempt(run(updateNotifications(instanceKey, serverId, channelId, patch)));
  const now = Date.now();
  const out: MenuEntry[] = [];
  if (isMuted(settings, now)) {
    out.push({
      id: "unmute",
      label: `Unmute ${what}`,
      icon: BellIcon,
      hint: mutedLabel(settings, now).replace(/^Muted ?/, ""),
      onSelect: () => change({ mutedUntil: false }),
    });
  } else {
    out.push({
      kind: "sub",
      id: "mute",
      label: `Mute ${what}`,
      icon: BellOffIcon,
      items: MUTE_FOR.map((m) => ({
        id: m.label,
        label: m.label,
        onSelect: () => change({ mutedUntil: m.ms === null ? null : new Date(Date.now() + m.ms) }),
      })),
    });
  }
  if (what === "channel") {
    const level = settings?.level ?? NotificationLevel.UNSPECIFIED;
    out.push({
      kind: "sub",
      id: "notify",
      label: "Notifications",
      icon: SlidersHorizontalIcon,
      hint: LEVELS.find((l) => l.value === level)?.short ?? "Server's",
      items: [
        { kind: "check", radio: true, id: "default", label: "Use the server's", checked: level === NotificationLevel.UNSPECIFIED, onSelect: () => change({ level: NotificationLevel.UNSPECIFIED }) },
        ...LEVELS.map((l) => ({ kind: "check" as const, radio: true, id: String(l.value), label: l.label, checked: level === l.value, onSelect: () => change({ level: l.value }) })),
      ],
    });
  } else if (what === "server") {
    out.push({ id: "notification-settings", label: "Notification settings", icon: BellRingIcon, onSelect: () => openSettings("server-notifications", serverId) });
  }
  return out;
}
