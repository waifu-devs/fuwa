import { PresenceStatus, type Presence } from "@/gen/fuwa/v1/presence_pb";
import type { Key } from "@/i18n/i18n";

/** How a presence shows: the dot's four looks, and the words for each. */

export type Shown = "online" | "idle" | "dnd" | "offline";

const SHOWN: Record<number, Shown> = {
  [PresenceStatus.ONLINE]: "online",
  [PresenceStatus.IDLE]: "idle",
  [PresenceStatus.DO_NOT_DISTURB]: "dnd",
};

export const STATUS_LABEL: Record<Shown | "invisible", Key> = {
  online: "workspace.presence.status.online",
  idle: "workspace.presence.status.idle",
  dnd: "workspace.presence.status.dnd",
  offline: "workspace.presence.status.offline",
  invisible: "workspace.presence.status.invisible",
};

export const shownOf = (presence: Presence | undefined): Shown => (presence ? (SHOWN[presence.status] ?? "offline") : "offline");
