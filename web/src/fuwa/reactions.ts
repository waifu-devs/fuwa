import { Code } from "@connectrpc/connect";
import { create } from "@bufbuild/protobuf";
import { ReactionSchema, type Message, type Reaction, type User } from "@/gen/fuwa/v1/types_pb";
import { dmEngine } from "@/e2ee/engine";
import { i18n } from "@/i18n/i18n";
import { NAMES_SHOWN, reactionKey, sameEmoji, toggled, toggledDm, type EmojiRef, type ReactionLike } from "@/lib/reactions";
import { displayName } from "@/lib/format";
import { reportUsage } from "@/lib/reports";
import { FuwaError, toFuwaError } from "./errors";
import { store, updateDms, updateInstance, withMessage, withUsers } from "./store";
import { engine } from "./sync";
import { dmProblem } from "./dms";

/*
 * Reacting to messages. In a server the instance keeps who reacted with
 * what; the store shows your reaction at once and puts it back if the
 * instance says no. In a direct message or a secure channel your device
 * sends the reaction inside the encryption, like a message, and every
 * device tallies them itself (e2ee/engine.ts, lib/reactions.ts).
 */

/** An emoji to react with: a standard one, or one of the server's own (with its name and whether it moves). */
export type ReactEmoji = EmojiRef & { emojiName?: string; animated?: boolean; emojiUrl?: string };

/** Why a reaction didn't go, in words: a full message has its own. */
function why(err: unknown): FuwaError {
  const e = toFuwaError(err);
  if (e.code === Code.ResourceExhausted) return new FuwaError({ code: e.code, message: i18n().t("chattools.reactions.full") });
  return e;
}

/** Your reaction on a message where it's loaded, on (`on`) or off. */
function showMine(instanceKey: string, message: Pick<Message, "id" | "channelId" | "threadId">, emoji: ReactEmoji, on: boolean) {
  const template = create(ReactionSchema, { emoji: emoji.emoji, emojiId: emoji.emojiId, emojiName: emoji.emojiName ?? "", animated: !!emoji.animated, emojiUrl: emoji.emojiUrl ?? "", count: 1, me: true });
  updateInstance(instanceKey, (i) =>
    withMessage(i, message.channelId, message.threadId, message.id, (m) => {
      const reactions = toggled(m.reactions, template, on);
      return reactions === m.reactions ? m : { ...m, reactions };
    }),
  );
}

/** Puts the instance's count for one emoji on a message, as its answer says. */
function showCount(instanceKey: string, message: Pick<Message, "id" | "channelId" | "threadId">, reaction: Reaction, on: boolean) {
  updateInstance(instanceKey, (i) =>
    withMessage(i, message.channelId, message.threadId, message.id, (m) => {
      const at = m.reactions.findIndex((r) => sameEmoji(r, reaction));
      if (reaction.count <= 0) return at === -1 ? m : { ...m, reactions: m.reactions.filter((_, n) => n !== at) };
      const next = { ...reaction, me: on };
      return { ...m, reactions: at === -1 ? [...m.reactions, next] : m.reactions.map((r, n) => (n === at ? next : r)) };
    }),
  );
}

/** Reacts to a message in a server with an emoji, or takes your reaction off. */
export async function react(instanceKey: string, serverId: string, message: Pick<Message, "id" | "channelId" | "threadId">, emoji: ReactEmoji, on: boolean) {
  reportUsage(on ? "message.react" : "message.unreact");
  showMine(instanceKey, message, emoji, on);
  try {
    const { reaction } = await engine(instanceKey).api.messages.react({
      serverId,
      channelId: message.channelId,
      messageId: message.id,
      emoji: emoji.emojiId ? "" : emoji.emoji,
      emojiId: emoji.emojiId,
      reacted: on,
    });
    if (reaction) showCount(instanceKey, message, reaction, on);
  } catch (err) {
    showMine(instanceKey, message, emoji, !on);
    throw why(err);
  }
}

/** One page of who reacted to a message with an emoji, the earliest first; their profiles go in the store. */
export async function listReactors(
  instanceKey: string,
  serverId: string,
  message: Pick<Message, "id" | "channelId">,
  emoji: EmojiRef,
  { limit = 50, afterId = "" }: { limit?: number; afterId?: string } = {},
): Promise<{ users: User[]; hasMore: boolean }> {
  try {
    const res = await engine(instanceKey).api.messages.listReactors({
      serverId,
      channelId: message.channelId,
      messageId: message.id,
      emoji: emoji.emojiId ? "" : emoji.emoji,
      emojiId: emoji.emojiId,
      limit,
      afterId,
    });
    updateInstance(instanceKey, (i) => {
      const users = withUsers(i.users, res.users);
      return users === i.users ? i : { ...i, users };
    });
    return { users: res.users, hasMore: res.hasMore };
  } catch (err) {
    throw toFuwaError(err);
  }
}

/** Who reacted, as loaded for the hover over a reaction, kept while its count and yours stay the same. */
const whoCache = new Map<string, Promise<string[]>>();
const WHO_KEPT = 200;

/**
 * The first few people who reacted to a message with an emoji, by the name
 * the server shows them by, `you` standing for yourself (first, when you're one).
 */
export function whoReacted(instanceKey: string, serverId: string, message: Pick<Message, "id" | "channelId">, reaction: ReactionLike, you: string): Promise<string[]> {
  const key = `${instanceKey}|${store.get().instances[instanceKey]?.me?.id}|${message.id}|${reactionKey(reaction)}|${reaction.count}|${reaction.me}`;
  let names = whoCache.get(key);
  if (!names) {
    names = listReactors(instanceKey, serverId, message, reaction, { limit: NAMES_SHOWN + 1 }).then(({ users }) => {
      const i = store.get().instances[instanceKey];
      const meId = i?.me?.id;
      const members = i?.members[serverId];
      const others = users.filter((u) => u.id !== meId).map((u) => members?.find((m) => m.user?.id === u.id)?.nickname || displayName(u));
      return reaction.me ? [you, ...others] : others;
    });
    names.catch(() => whoCache.delete(key));
    whoCache.set(key, names);
    if (whoCache.size > WHO_KEPT) whoCache.delete(whoCache.keys().next().value!);
  }
  return names;
}

/** Takes one emoji's reactions off a message, or every reaction (no emoji): moderators only. */
export async function clearReactions(instanceKey: string, serverId: string, message: Pick<Message, "id" | "channelId" | "threadId">, emoji?: EmojiRef) {
  reportUsage(emoji ? "message.reactions_clear_one" : "message.reactions_clear");
  try {
    await engine(instanceKey).api.messages.clearReactions({
      serverId,
      channelId: message.channelId,
      messageId: message.id,
      emoji: emoji && !emoji.emojiId ? emoji.emoji : "",
      emojiId: emoji?.emojiId ?? "",
    });
  } catch (err) {
    throw toFuwaError(err);
  }
  updateInstance(instanceKey, (i) =>
    withMessage(i, message.channelId, message.threadId, message.id, (m) => ({
      ...m,
      reactions: emoji ? m.reactions.filter((r) => !sameEmoji(r, emoji)) : [],
    })),
  );
}

/**
 * Reacts to a line in a direct message or secure channel (standard emoji
 * only), or takes your reaction off: sent encrypted to the conversation's
 * devices. Shown at once; put back if it couldn't be sent.
 */
export async function reactInConversation(instanceKey: string, conversationId: string, seq: number, emoji: string, on: boolean) {
  reportUsage(on ? "dm.react" : "dm.unreact");
  const me = store.get().instances[instanceKey]?.me?.id ?? "";
  updateDms(instanceKey, (d) => {
    const all = d.reactions[conversationId] ?? {};
    const list = all[seq] ?? [];
    const next = toggledDm(list, emoji, me, on);
    return next === list ? d : { ...d, reactions: { ...d.reactions, [conversationId]: { ...all, [seq]: next } } };
  });
  const dms = dmEngine(instanceKey);
  try {
    if (!dms) throw new Error(i18n().t("system.e2ee.starting"));
    await dms.send(conversationId, { react: seq, emoji, removed: !on });
  } catch (err) {
    await dms?.refresh(conversationId).catch(() => {});
    throw new FuwaError({ code: Code.Unknown, message: dmProblem(err) });
  }
}
