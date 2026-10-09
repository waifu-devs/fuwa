/**
 * Which events an agent's endpoint gets (docs/agent-endpoints.md), by their
 * name in `Event`'s payload ("message_created"). The names come from the
 * generated `EventSchema`, so new events show up here without a change; this
 * only sorts them into groups people recognise. Pure, for tests.
 */

/** Events the instance never stores, so it never posts them to an endpoint. */
export const UNDELIVERED: ReadonlySet<string> = new Set(["voice_state_updated", "voice_state_removed", "live_tile_updated", "live_tile_ended"]);

export type EventGroup = "messages" | "interactions" | "members" | "reactions" | "other";

/** The order groups are shown in. */
export const GROUPS: readonly EventGroup[] = ["messages", "interactions", "members", "reactions", "other"];

const GROUP_OF: Record<string, EventGroup> = {
  message_created: "messages",
  message_updated: "messages",
  message_deleted: "messages",
  message_pinned: "messages",
  thread_updated: "messages",
  poll_updated: "messages",
  interaction_created: "interactions",
  member_joined: "members",
  member_left: "members",
  member_updated: "members",
  reaction_updated: "reactions",
  reactions_cleared: "reactions",
};

/** The group an event goes under: anything not named above is "other". */
export const groupOf = (name: string): EventGroup => GROUP_OF[name] ?? "other";

/** The names worth offering: what an endpoint can get, plus any already chosen (so they can be taken off). */
export function offered(names: readonly string[], chosen: readonly string[] = []): string[] {
  const out = names.filter((n) => !UNDELIVERED.has(n));
  for (const n of chosen) if (!out.includes(n)) out.push(n);
  return out;
}

/** The names in their groups, in `GROUPS` order, leaving out empty groups; names keep their order. */
export function grouped(names: readonly string[]): { group: EventGroup; names: string[] }[] {
  return GROUPS.map((group) => ({ group, names: names.filter((n) => groupOf(n) === group) })).filter((g) => g.names.length > 0);
}

/** A name as words, for events without a label of their own: "shared_channels_updated" → "Shared channels updated". */
export function readable(name: string): string {
  const words = name.replace(/_+/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** Adds the name, or takes it off when it's there. */
export const toggle = (chosen: readonly string[], name: string): string[] =>
  chosen.includes(name) ? chosen.filter((n) => n !== name) : [...chosen, name];

/** Adds every name of a group, or takes them all off when they're all there already. */
export function toggleAll(chosen: readonly string[], names: readonly string[]): string[] {
  const all = names.every((n) => chosen.includes(n));
  return all ? chosen.filter((n) => !names.includes(n)) : [...chosen, ...names.filter((n) => !chosen.includes(n))];
}

/** Whether two choices are the same events, in any order. */
export function sameEvents(a: readonly string[], b: readonly string[]): boolean {
  const left = new Set(a);
  const right = new Set(b);
  return left.size === right.size && [...left].every((n) => right.has(n));
}
