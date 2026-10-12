import type { Channel, Message, ModCase, ModNote } from "@/gen/fuwa/v1/types_pb";
import type { Key } from "../i18n/i18n.ts";

/*
 * Mod View (docs/mod-view.md): the small decisions its screens share, from
 * what may be reported to keeping a list of cases in step with the events
 * that change them. Only type imports, so node's test runner reads this file
 * as is; the enums it needs are written out below, as the protocol numbers
 * them.
 */

/** fuwa.v1.ModCaseStatus. */
export const OPEN = 1;
export const RESOLVED = 2;
export const DISMISSED = 3;

/** fuwa.v1.ModCaseKind. */
export const AUTO_MOD = 1;
export const MESSAGE_REPORT = 2;
export const MEMBER_REPORT = 3;

/** fuwa.v1.ChannelType.SECURE and MessageKind.UNSPECIFIED. */
const SECURE = 6;
const PLAIN_MESSAGE = 0;

/** As the server takes them: a report's note, a reason for closing a case, a note about someone. */
export const REPORT_NOTE_MAX = 500;
export const RESOLUTION_MAX = 500;
export const NOTE_MAX = 1000;

/** fuwa.v1.ModReportReason, in the order the report dialog offers them, with their words. */
export const REASONS: { value: number; label: Key; about: Key }[] = [
  { value: 1, label: "chattools.modview.reason.spam", about: "chattools.modview.reason.spamAbout" },
  { value: 2, label: "chattools.modview.reason.harassment", about: "chattools.modview.reason.harassmentAbout" },
  { value: 3, label: "chattools.modview.reason.hate", about: "chattools.modview.reason.hateAbout" },
  { value: 4, label: "chattools.modview.reason.sexual", about: "chattools.modview.reason.sexualAbout" },
  { value: 5, label: "chattools.modview.reason.violence", about: "chattools.modview.reason.violenceAbout" },
  { value: 6, label: "chattools.modview.reason.selfHarm", about: "chattools.modview.reason.selfHarmAbout" },
  { value: 7, label: "chattools.modview.reason.scam", about: "chattools.modview.reason.scamAbout" },
  { value: 8, label: "chattools.modview.reason.rules", about: "chattools.modview.reason.rulesAbout" },
  { value: 9, label: "chattools.modview.reason.other", about: "chattools.modview.reason.otherAbout" },
];

/** A reason's name; one this app doesn't know reads as "Something else". */
export const reasonLabel = (value: number): Key => REASONS.find((r) => r.value === value)?.label ?? "chattools.modview.reason.other";

/** A case's kind, in words. */
export const kindLabel = (kind: number): Key =>
  kind === AUTO_MOD ? "chattools.modview.kind.autoMod" : kind === MEMBER_REPORT ? "chattools.modview.kind.memberReport" : "chattools.modview.kind.messageReport";

/** A case's state, in words. */
export const statusLabel = (status: number): Key =>
  status === RESOLVED ? "chattools.modview.status.resolved" : status === DISMISSED ? "chattools.modview.status.dismissed" : "chattools.modview.status.open";

/** The queue's tabs, each a section of the Mod View screen. */
export const TABS = ["open", "resolved", "dismissed", "all"] as const;
export type QueueTab = (typeof TABS)[number];

export const isTab = (id: string): id is QueueTab => (TABS as readonly string[]).includes(id);

/** The state a tab lists; 0 (every state) for All. */
export const statusOfTab = (tab: QueueTab): number => (tab === "open" ? OPEN : tab === "resolved" ? RESOLVED : tab === "dismissed" ? DISMISSED : 0);

/** What a list of cases shows: a state (0 for any) and, optionally, one person. */
export type CaseFilter = { status: number; subjectId: string };

export const caseMatches = (c: Pick<ModCase, "status" | "subjectId">, filter: CaseFilter) =>
  (!filter.status || c.status === filter.status) && (!filter.subjectId || c.subjectId === filter.subjectId);

/**
 * A list of cases (newest first, as the server pages them) after one of them
 * changed: replaced where it is, dropped once it no longer belongs, or put in
 * its place when it now does. One older than every case loaded so far waits
 * for its page, unless the list has nothing more to load (`complete`).
 */
export function withCase(list: readonly ModCase[], changed: ModCase, filter: CaseFilter, complete: boolean): ModCase[] {
  const at = list.findIndex((c) => c.id === changed.id);
  const belongs = caseMatches(changed, filter);
  if (at !== -1) return belongs ? list.map((c, n) => (n === at ? changed : c)) : list.filter((_, n) => n !== at);
  if (!belongs) return [...list];
  const last = list.at(-1);
  if (last && changed.id < last.id && !complete) return [...list];
  const next = [...list, changed];
  next.sort((a, b) => (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));
  return next;
}

/** Notes about someone, newest first, with one added (once). */
export function withNote(list: readonly ModNote[], note: ModNote): ModNote[] {
  if (list.some((n) => n.id === note.id)) return [...list];
  return [note, ...list].sort((a, b) => (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));
}

/**
 * Whether you may report a message, as the server decides: one someone else
 * wrote, an ordinary message, in a channel the server keeps and can read
 * (never a secure channel, nor one shared from another server).
 */
export function mayReport(
  channel: Pick<Channel, "type" | "shared"> | undefined,
  message: Pick<Message, "kind" | "authorId">,
  meId: string | undefined,
): boolean {
  if (!channel || !meId) return false;
  if (channel.type === SECURE) return false;
  if (channel.shared && !channel.shared.home) return false;
  return message.kind === PLAIN_MESSAGE && message.authorId !== meId && !!message.authorId;
}

/** A case's text on one line, cut to fit a row in the queue. */
export function snippet(content: string, max = 140): string {
  const line = content.replace(/\s+/g, " ").trim();
  return line.length > max ? `${line.slice(0, max - 1).trimEnd()}…` : line;
}

/** How many whole days an account has been around. */
export const daysSince = (created: Date, now: number) => Math.max(0, Math.floor((now - created.getTime()) / 86_400_000));
