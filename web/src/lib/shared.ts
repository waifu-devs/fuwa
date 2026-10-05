import type { Channel, Message, SharedServer } from "@/gen/fuwa/v1/types_pb";
import type { Lang } from "./durations.ts";

/*
 * Shared channels (like Slack Connect): one server, the channel's home, keeps
 * the channel and its messages; another shows it as a channel of its own.
 * These are the small decisions the screens share. Only type imports, so
 * node's test runner reads this file as is.
 */

/**
 * A share code is the home server's id, a dash and 16 letters and digits, then "@" and
 * the home's instance when it's for servers on other instances too. People paste
 * them out of chats, so look for one anywhere in the text. Returns "" when there's none.
 */
export function findShareCode(text: string): string {
  return SHARE_CODE.exec(text)?.[0] ?? "";
}

const SHARE_CODE = /[0-9A-HJKMNP-TV-Z]{26}-[A-Za-z0-9]{16}(?:@(?:https?:\/\/)?(?:\[[0-9A-Fa-f:.]+\]|[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?)(?::\d{1,5})?)?/i;

/** The instance a share code names, as people read it, or "" for a code for this instance only. */
export function shareCodeInstance(code: string): string {
  const at = code.indexOf("@");
  return at < 0 ? "" : code.slice(at + 1).replace(/^https:\/\//, "");
}

/** Who's on the other end of a shared channel, as the sidebar and header say it. */
export function sharedLabel(channel: Pick<Channel, "shared"> | undefined): { home: boolean; names: string; text: string } | null {
  const shared = channel?.shared;
  if (!shared) return null;
  if (shared.home) {
    const names = listNames(shared.guests.map((g) => g.name));
    return { home: true, names, text: names ? `Shared with ${names}` : "Shared" };
  }
  const names = shared.homeServer?.name ?? "";
  return { home: false, names, text: names ? `Shared from ${names}` : "Shared" };
}

/** "A", "A and B", "A, B and C". */
export function listNames(names: string[]): string {
  const clean = names.filter(Boolean);
  if (clean.length <= 1) return clean[0] ?? "";
  return `${clean.slice(0, -1).join(", ")} and ${clean.at(-1)}`;
}

/**
 * The server a message's author is from, when it isn't the one it's read in:
 * at the home that's set only for guests' messages; at a guest it's set on
 * every message, so the guest's own people are left untagged.
 */
export function foreignServer(message: Pick<Message, "shared">, serverId: string): SharedServer | null {
  const server = message.shared?.server;
  return server && server.id && server.id !== serverId ? server : null;
}

/** A number of days, hours or minutes, in the language's own words. */
const units = (locale: string, n: number, unit: "day" | "hour" | "minute") =>
  new Intl.NumberFormat(locale, { style: "unit", unit, unitDisplay: "long" }).format(n);

/** How long a share code has left, in words and in the app's language: "6 days", "3 hours", "a few minutes". */
export function codeLeft({ t, locale }: Lang, ms: number): string {
  if (ms <= 0) return t("serversettings.sharedChannels.expired");
  const hours = ms / 3_600_000;
  // A code made a moment ago "works for 7 days", not 6 and a bit.
  if (hours >= 36) return units(locale, Math.round(hours / 24), "day");
  if (hours >= 2) return units(locale, Math.floor(hours), "hour");
  const minutes = Math.floor(ms / 60_000);
  return minutes >= 5 ? units(locale, minutes, "minute") : t("serversettings.sharedChannels.fewMinutes");
}
