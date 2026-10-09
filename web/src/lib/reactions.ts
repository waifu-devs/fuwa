/**
 * Reactions to messages, apart from React and the store so node's test
 * runner reads this file as is.
 *
 * In a server, the instance keeps them: a message comes with its reactions
 * (`Message.reactions`, `me` set for the reader), and ReactionUpdated and
 * ReactionsCleared keep them current. Events can arrive twice, so a count is
 * always taken from the event, never added to.
 *
 * In a direct message or a secure channel nobody but the devices knows who
 * reacted: each reaction is a message of its own (`DirectMessageReaction`),
 * and each device tallies them, the latest from a sender for an emoji
 * winning (`tallyDm`).
 */

/** What every app reads of a reaction: `Reaction` from the protocol, or a direct message's tally. */
export type ReactionLike = { emoji: string; emojiId: string; emojiName: string; animated: boolean; count: number; me: boolean };

/** Which emoji: a standard one by its characters, or a server's own by its id. */
export type EmojiRef = { emoji: string; emojiId: string };

/** One key per emoji, for lists and lookups. */
export const reactionKey = (r: EmojiRef) => (r.emojiId ? `c:${r.emojiId}` : `u:${r.emoji}`);

export const sameEmoji = (a: EmojiRef, b: EmojiRef) => reactionKey(a) === reactionKey(b);

/**
 * A message's reactions after a ReactionUpdated: the emoji's count as the
 * event says (gone at 0), and `me` from it when it's about you (`mine`:
 * true when you reacted, false when you took yours off, undefined when it was
 * someone else), else as it was. A new emoji goes last, as it was first used
 * last. Arriving twice changes nothing.
 */
export function withReactionUpdate<R extends ReactionLike>(list: readonly R[], update: R, mine: boolean | undefined): R[] {
  const at = list.findIndex((r) => sameEmoji(r, update));
  if (update.count <= 0) return at === -1 ? (list as R[]) : list.filter((_, n) => n !== at);
  if (at === -1) return [...list, { ...update, me: mine ?? false }];
  const before = list[at]!;
  const me = mine ?? before.me;
  if (before.count === update.count && before.me === me) return list as R[];
  return list.map((r, n) => (n === at ? { ...before, count: update.count, me } : r));
}

/** A message's reactions after a moderator cleared one emoji's, or every one (both empty). */
export function withoutReactions<R extends ReactionLike>(list: readonly R[], cleared: EmojiRef): R[] {
  if (!cleared.emoji && !cleared.emojiId) return list.length ? [] : (list as R[]);
  const next = list.filter((r) => !sameEmoji(r, cleared));
  return next.length === list.length ? (list as R[]) : next;
}

/**
 * Your own reaction put on (`on`) or taken off, as the app shows it before
 * the instance answers. `template` is the emoji, used when it's new here.
 */
export function toggled<R extends ReactionLike>(list: readonly R[], template: R, on: boolean): R[] {
  const at = list.findIndex((r) => sameEmoji(r, template));
  const before = at === -1 ? undefined : list[at]!;
  if (on) {
    if (before?.me) return list as R[];
    if (!before) return [...list, { ...template, count: 1, me: true }];
    return list.map((r, n) => (n === at ? { ...before, count: before.count + 1, me: true } : r));
  }
  if (!before?.me) return list as R[];
  if (before.count <= 1) return list.filter((_, n) => n !== at);
  return list.map((r, n) => (n === at ? { ...before, count: before.count - 1, me: false } : r));
}

/** Whether you reacted with this emoji already. */
export const reactedWith = (list: readonly ReactionLike[] | undefined, emoji: EmojiRef) => !!list?.some((r) => r.me && sameEmoji(r, emoji));

// ───────────────────────── Who reacted ─────────────────────────

/** How many names a hover over a reaction shows before "and N more". */
export const NAMES_SHOWN = 3;

/**
 * "Ana, Bo and 3 more", from the first names and how many reacted in all.
 * `and` joins the last two of a short list; `more` says how many aren't named.
 */
export function peopleLine(names: string[], total: number, words: { separator: string; and: (rest: string, last: string) => string; more: (count: number) => string }): string {
  const shown = names.filter(Boolean).slice(0, NAMES_SHOWN);
  const rest = Math.max(0, total - shown.length);
  if (!shown.length) return rest ? words.more(rest) : "";
  if (rest > 0) return `${shown.join(words.separator)} ${words.more(rest)}`;
  if (shown.length === 1) return shown[0]!;
  return words.and(shown.slice(0, -1).join(words.separator), shown.at(-1)!);
}

// ───────────────────────── Direct messages ─────────────────────────

/** Most a standard emoji's characters take, as the protocol says. */
const MAX_EMOJI_BYTES = 32;
const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" });

/** Whether `text` is one standard emoji, as a reaction must be. */
export function isOneEmoji(text: string): boolean {
  if (!text || new TextEncoder().encode(text).length > MAX_EMOJI_BYTES) return false;
  const parts = [...graphemes.segment(text)];
  if (parts.length !== 1) return false;
  return /\p{Extended_Pictographic}|\p{Regional_Indicator}|⃣/u.test(text);
}

/** What a direct message's line says about a reaction, as the device kept it. */
export type DmReactionRef = { target: number; emoji: string; removed: boolean };

/** What tallying reads of each line in a conversation. */
export type TallyLine = { seq: number; senderId: string; kind: string; deleted: boolean; reaction?: DmReactionRef };

/** A reaction in a direct message or secure channel: who reacted, in the order they did. */
export type DmReaction = ReactionLike & { userIds: string[] };

/** Lines a reaction can be to: messages that are still there. */
const reactable = (line: TallyLine | undefined) => !!line && (line.kind === "text" || line.kind === "voice") && !line.deleted;

/**
 * Each message's reactions, from the reaction lines a device opened. For one
 * sender, message and emoji the latest line wins (adding again, or taking
 * off). Reactions to lines that aren't messages, or are gone, are left out.
 * Emoji come in the order they were first used among those still on, and
 * people in the order they reacted.
 */
export function tallyDm(lines: readonly TallyLine[], me: string): Record<number, DmReaction[]> {
  const bySeq = new Map(lines.map((l) => [l.seq, l]));
  const latest = new Map<string, { target: number; senderId: string; emoji: string; seq: number; on: boolean }>();
  for (const line of [...lines].sort((a, b) => a.seq - b.seq)) {
    const r = line.reaction;
    if (line.kind !== "reaction" || !r || line.deleted || r.target <= 0 || r.target >= line.seq || !isOneEmoji(r.emoji)) continue;
    latest.set(`${r.target}\u0000${line.senderId}\u0000${r.emoji}`, { target: r.target, senderId: line.senderId, emoji: r.emoji, seq: line.seq, on: !r.removed });
  }
  const on = [...latest.values()].filter((v) => v.on && reactable(bySeq.get(v.target))).sort((a, b) => a.seq - b.seq);
  const out: Record<number, DmReaction[]> = {};
  for (const v of on) {
    const list = (out[v.target] ??= []);
    let reaction = list.find((r) => r.emoji === v.emoji);
    if (!reaction) list.push((reaction = { emoji: v.emoji, emojiId: "", emojiName: "", animated: false, count: 0, me: false, userIds: [] }));
    reaction.userIds.push(v.senderId);
    reaction.count++;
    if (v.senderId === me) reaction.me = true;
  }
  return out;
}

/** A direct message's reactions with yours put on or taken off, before the device has sent it. */
export function toggledDm(list: readonly DmReaction[], emoji: string, me: string, on: boolean): DmReaction[] {
  const template: DmReaction = { emoji, emojiId: "", emojiName: "", animated: false, count: 1, me: true, userIds: [me] };
  const next = toggled(list, template, on);
  if (next === list) return next;
  return next.map((r) => {
    if (r.emoji !== emoji) return r;
    const others = r.userIds.filter((id) => id !== me);
    return { ...r, userIds: on ? [...others, me] : others };
  });
}
