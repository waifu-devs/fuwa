import { LoaderCircleIcon, PlusIcon, SmilePlusIcon, Trash2Icon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useId, useState, type ReactElement } from "react";
import type { Emoji, User } from "@/gen/fuwa/v1/types_pb";
import { EmojiImage } from "@/components/EmojiImage";
import { EmojiPicker, type PickedEmoji } from "@/components/EmojiPicker";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { ReactEmoji } from "@/fuwa/reactions";
import { useI18n } from "@/i18n/react";
import { recentEmoji, rememberEmoji, type Catalog } from "@/lib/emoji-catalog";
import { displayName } from "@/lib/format";
import { SPRING } from "@/lib/motion";
import { peopleLine, reactionKey, type ReactionLike } from "@/lib/reactions";
import { cn } from "@/lib/utils";

/*
 * Reactions under a message: a chip per emoji with how many reacted, lit
 * when you're one of them. Pressing a chip adds yours or takes it off;
 * hovering says who. The same row serves channels, threads, direct messages
 * and secure channels; where the people come from is the caller's.
 */

/** A catalog with nothing in it: direct messages and secure channels react with standard emoji only. */
export const NO_CATALOG: Catalog = { sections: [], byAlias: new Map(), byId: new Map() };

/** What a server's messages may be reacted with: the standard set and the server's own emoji (no other server's). */
export function reactionCatalog(catalog: Catalog): Catalog {
  const sections = catalog.sections.filter((s) => s.here);
  if (sections.length === catalog.sections.length) return catalog;
  const own = sections.flatMap((s) => s.emojis);
  return { sections, byAlias: new Map(own.map((e) => [e.alias.toLowerCase(), e])), byId: new Map(own.map((e) => [e.emoji.id, e])) };
}

/** A picked emoji as a reaction: a standard one, or one of the server's own. */
export const pickedReaction = (picked: PickedEmoji): ReactEmoji =>
  picked.custom
    ? { emoji: "", emojiId: picked.custom.emoji.id, emojiName: picked.custom.emoji.name, animated: picked.custom.emoji.animated }
    : { emoji: picked.text, emojiId: "" };

/** How a reaction's emoji reads in words: itself, or `:name:` for a server's. */
export const emojiLabel = (r: Pick<ReactionLike, "emoji" | "emojiName">) => r.emoji || `:${r.emojiName}:`;

/** Emoji offered first when nothing's been used yet. */
const STARTERS = ["👍", "❤️", "😂", "😮", "😢", "🎉"];
const QUICK = 6;

/**
 * The emoji a message's menu offers in one press: the ones used lately that
 * can go here (`emojis`, a server's own; none in direct messages), then the
 * usual few.
 */
export function quickReactions(emojis: Emoji[] | null): ReactEmoji[] {
  const out: ReactEmoji[] = [];
  const seen = new Set<string>();
  const add = (r: ReactEmoji) => {
    const key = reactionKey(r);
    if (seen.has(key) || out.length >= QUICK) return;
    seen.add(key);
    out.push(r);
  };
  for (const key of recentEmoji()) {
    if (key.startsWith("u:")) add({ emoji: key.slice(2), emojiId: "" });
    else if (key.startsWith("c:") && emojis) {
      const e = emojis.find((x) => x.id === key.slice(2));
      if (e) add({ emoji: "", emojiId: e.id, emojiName: e.name, animated: e.animated });
    }
  }
  for (const char of STARTERS) add({ emoji: char, emojiId: "" });
  return out;
}

/** A reaction's emoji: a standard one as text, a server's own as its picture (its name when it's gone). */
export function ReactionEmoji({ reaction, emojis, className }: { reaction: Pick<ReactionLike, "emoji" | "emojiId" | "emojiName" | "animated">; emojis?: Emoji[]; className?: string }) {
  if (!reaction.emojiId) return <span className={cn("inline-grid place-items-center leading-none", className)}>{reaction.emoji}</span>;
  const emoji = emojis?.find((e) => e.id === reaction.emojiId);
  if (!emoji) return <span className={cn("text-[0.7rem] text-muted-foreground", className)}>:{reaction.emojiName}:</span>;
  return <EmojiImage emoji={emoji} className={className} title={false} />;
}

/** Who reacted with one emoji, as names to show first ("You" for yourself), or a promise of them. */
export type WhoReacted = (reaction: ReactionLike) => string[] | Promise<string[]>;

/**
 * A message's reactions, and a "+" to add one. `canAdd` is whether you may
 * react here (taking yours off is always allowed). Chips spring in and out,
 * counts roll.
 */
export function ReactionRow({
  reactions,
  emojis,
  canAdd,
  catalog,
  onToggle,
  onPick,
  who,
}: {
  reactions: readonly ReactionLike[];
  emojis?: Emoji[];
  canAdd: boolean;
  catalog: Catalog;
  onToggle: (reaction: ReactionLike, on: boolean) => void;
  onPick: (picked: PickedEmoji) => void;
  who: WhoReacted;
}) {
  const { t } = useI18n();
  if (!reactions.length) return null;
  return (
    <motion.div layout="position" transition={SPRING} className="mt-1 flex flex-wrap items-center gap-1" role="group" aria-label={t("chattools.reactions.title")}>
      <AnimatePresence initial={false}>
        {reactions.map((r) => (
          <ReactionChip key={reactionKey(r)} reaction={r} emojis={emojis} canAdd={canAdd} onToggle={onToggle} who={who} />
        ))}
        {canAdd && (
          <motion.span key="add" layout="position" transition={SPRING} className="inline-flex">
            <EmojiPicker catalog={catalog} onPick={onPick} placement="top-start">
              {(open) => (
                <motion.button
                  type="button"
                  whileTap={{ scale: 0.88 }}
                  aria-label={t("chattools.reactions.add")}
                  title={t("chattools.reactions.add")}
                  className={cn(
                    "reaction-add grid h-7 w-8 place-items-center rounded-lg border border-transparent text-muted-foreground transition hover:border-border hover:bg-muted hover:text-foreground",
                    open && "border-border bg-muted text-foreground",
                  )}
                >
                  <SmilePlusIcon className="size-4" />
                </motion.button>
              )}
            </EmojiPicker>
          </motion.span>
        )}
      </AnimatePresence>
    </motion.div>
  );
}

function ReactionChip({
  reaction: r,
  emojis,
  canAdd,
  onToggle,
  who,
}: {
  reaction: ReactionLike;
  emojis?: Emoji[];
  canAdd: boolean;
  onToggle: (reaction: ReactionLike, on: boolean) => void;
  who: WhoReacted;
}) {
  const { t } = useI18n();
  const able = r.me || canAdd;
  return (
    <motion.span
      layout="position"
      initial={{ opacity: 0, scale: 0.5 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.5 }}
      transition={{ type: "spring", stiffness: 600, damping: 26 }}
      className="inline-flex"
    >
      <Tooltip>
        <TooltipTrigger asChild>
          <motion.button
            type="button"
            aria-pressed={r.me}
            aria-disabled={!able}
            aria-label={t("chattools.reactions.who", { emoji: emojiLabel(r) })}
            whileTap={able ? { scale: 0.88 } : undefined}
            onClick={() => able && onToggle(r, !r.me)}
            className={cn(
              "flex h-7 items-center gap-1.5 rounded-lg border px-2 text-xs font-bold transition-colors",
              r.me ? "border-primary/50 bg-primary/15 text-primary hover:bg-primary/20" : "border-transparent bg-muted/70 text-muted-foreground hover:border-border hover:bg-muted",
              !able && "cursor-default",
            )}
          >
            <motion.span
              key={r.me ? "on" : "off"}
              initial={r.me ? { scale: 0.4, rotate: -20 } : false}
              animate={{ scale: 1, rotate: 0 }}
              transition={{ type: "spring", stiffness: 600, damping: 14 }}
              className="inline-grid"
            >
              <ReactionEmoji reaction={r} emojis={emojis} className="size-4 text-[0.95rem]" />
            </motion.span>
            <Count value={r.count} />
          </motion.button>
        </TooltipTrigger>
        <TooltipContent side="top" className="max-w-64">
          <WhoTip reaction={r} emojis={emojis} who={who} />
        </TooltipContent>
      </Tooltip>
    </motion.span>
  );
}

/** "You, Ana and 3 more reacted with 👍", loaded when it's first shown. */
function WhoTip({ reaction, emojis, who }: { reaction: ReactionLike; emojis?: Emoji[]; who: WhoReacted }) {
  const { t } = useI18n();
  const [names, setNames] = useState<string[] | null | "failed">(() => {
    const now = who(reaction);
    return Array.isArray(now) ? now : null;
  });
  useEffect(() => {
    const now = who(reaction);
    if (Array.isArray(now)) return setNames(now);
    let live = true;
    now.then(
      (list) => live && setNames(list),
      () => live && setNames("failed"),
    );
    return () => {
      live = false;
    };
  }, [who, reaction]);
  return (
    <span className="flex items-center gap-2">
      <ReactionEmoji reaction={reaction} emojis={emojis} className="size-6 shrink-0 text-xl" />
      <span>
        {names === null
          ? t("chattools.reactions.loading")
          : names === "failed"
            ? t("chattools.reactions.failed")
            : t("chattools.reactions.tooltip", {
                people: peopleLine(names, reaction.count, {
                  separator: t("chat.shared.namesSeparator"),
                  and: (rest, last) => t("chat.shared.names", { rest, last }),
                  more: (count) => t("chattools.reactions.andMore", { count }),
                }),
                emoji: emojiLabel(reaction),
              })}
      </span>
    </span>
  );
}

/** The toolbar's "Add reaction" over a hovered message: opens the picker there, also when a menu asks. */
export function AddReactionTool({
  catalog,
  open,
  onOpenChange,
  onPick,
  children,
}: {
  catalog: Catalog;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onPick: (picked: PickedEmoji) => void;
  children: ReactElement;
}) {
  return (
    <EmojiPicker catalog={catalog} onPick={onPick} open={open} onOpenChange={onOpenChange} placement="bottom-end">
      {() => children}
    </EmojiPicker>
  );
}

/** The row of emoji at the top of a message's menu: one press reacts (or takes yours off), and the menu closes. */
export function QuickReactions({
  choices,
  reactions,
  emojis,
  canAdd,
  onToggle,
  close,
}: {
  choices: ReactEmoji[];
  reactions: readonly ReactionLike[];
  emojis?: Emoji[];
  canAdd: boolean;
  onToggle: (emoji: ReactEmoji, on: boolean) => void;
  close: () => void;
}) {
  const { t } = useI18n();
  const mine = new Set(reactions.filter((r) => r.me).map(reactionKey));
  return (
    <div role="group" aria-label={t("chattools.reactions.react")} className="flex items-center justify-between gap-0.5 px-1 pb-1">
      {choices.map((c, n) => {
        const on = mine.has(reactionKey(c));
        if (!on && !canAdd) return null;
        return (
          <motion.button
            key={reactionKey(c)}
            type="button"
            initial={{ opacity: 0, scale: 0.6, y: -3 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            transition={{ type: "spring", stiffness: 700, damping: 22, delay: n * 0.02 }}
            whileHover={{ scale: 1.18 }}
            whileTap={{ scale: 0.85 }}
            aria-pressed={on}
            aria-label={emojiLabel({ emoji: c.emoji, emojiName: c.emojiName ?? "" })}
            title={emojiLabel({ emoji: c.emoji, emojiName: c.emojiName ?? "" })}
            onClick={() => {
              if (!on) rememberEmoji(c.emojiId ? `c:${c.emojiId}` : `u:${c.emoji}`);
              onToggle(c, !on);
              close();
            }}
            className={cn("grid size-8 place-items-center rounded-lg text-lg transition-colors hover:bg-muted", on && "bg-primary/15 ring-1 ring-primary/40")}
          >
            <ReactionEmoji reaction={{ emoji: c.emoji, emojiId: c.emojiId, emojiName: c.emojiName ?? "", animated: !!c.animated }} emojis={emojis} className="size-5" />
          </motion.button>
        );
      })}
    </div>
  );
}

/**
 * Everyone who reacted to a message, an emoji at a time. Moderators can take
 * one emoji's reactions off from here.
 */
export function ReactionsDialog({
  open,
  onOpenChange,
  reactions,
  emojis,
  load,
  onClear,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  reactions: readonly ReactionLike[];
  emojis?: Emoji[];
  /** A page of who reacted with an emoji, after the person `afterId`. */
  load: (reaction: ReactionLike, afterId: string) => Promise<{ users: User[]; hasMore: boolean }>;
  /** Takes every reaction with this emoji off (moderators). */
  onClear?: (reaction: ReactionLike) => Promise<void>;
}) {
  const { t, number } = useI18n();
  const [chosen, setChosen] = useState("");
  const current = reactions.find((r) => reactionKey(r) === chosen) ?? reactions[0];
  const id = useId();
  // Every reaction gone (a moderator cleared the last one): nothing left to show.
  const empty = open && reactions.length === 0;
  useEffect(() => {
    if (empty) onOpenChange(false);
  }, [empty, onOpenChange]);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader title={t("chattools.reactions.title")} />
        <div role="tablist" aria-label={t("chattools.reactions.title")} className="scroll-thin -mx-1 mb-3 flex gap-1.5 overflow-x-auto px-1 pb-1">
          {reactions.map((r) => {
            const active = current && reactionKey(r) === reactionKey(current);
            return (
              <button
                key={reactionKey(r)}
                type="button"
                role="tab"
                aria-selected={!!active}
                onClick={() => setChosen(reactionKey(r))}
                className={cn(
                  "relative flex shrink-0 items-center gap-1.5 rounded-full border px-3 py-1.5 text-xs font-bold transition-colors",
                  active ? "border-transparent text-primary-foreground" : "text-muted-foreground hover:text-foreground",
                )}
              >
                {active && <motion.span layoutId={`reactions-${id}`} transition={SPRING} className="absolute inset-0 rounded-full bg-primary" />}
                <ReactionEmoji reaction={r} emojis={emojis} className="relative size-4" />
                <span className="relative tabular-nums opacity-80">{number(r.count)}</span>
              </button>
            );
          })}
        </div>
        {open && current && <ReactorList key={reactionKey(current)} reaction={current} load={load} />}
        {open && current && onClear && <ClearOne reaction={current} onClear={onClear} />}
      </DialogContent>
    </Dialog>
  );
}

function ReactorList({ reaction, load }: { reaction: ReactionLike; load: (reaction: ReactionLike, afterId: string) => Promise<{ users: User[]; hasMore: boolean }> }) {
  const { t } = useI18n();
  const [users, setUsers] = useState<User[] | null>(null);
  const [more, setMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Loaded again when the count changes while it's open.
  const count = reaction.count;
  useEffect(() => {
    let live = true;
    load(reaction, "").then(
      (res) => {
        if (!live) return;
        setUsers(res.users);
        setMore(res.hasMore);
      },
      () => live && setError(t("chattools.reactions.failed")),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the reaction object changes with every event; its count is what matters
  }, [load, count, t]);
  async function loadMore() {
    const last = users?.at(-1);
    if (!last) return;
    try {
      const res = await load(reaction, last.id);
      setUsers((list) => [...(list ?? []), ...res.users.filter((u) => !list?.some((x) => x.id === u.id))]);
      setMore(res.hasMore);
    } catch {
      setError(t("chattools.reactions.failed"));
    }
  }
  if (error) return <p className="text-sm text-destructive">{error}</p>;
  if (!users) return <LoaderCircleIcon className="mx-auto my-6 size-5 animate-spin text-muted-foreground" aria-label={t("chattools.reactions.loading")} />;
  return (
    <ul className="scroll-thin flex max-h-80 flex-col gap-1 overflow-y-auto">
      {users.map((user, n) => (
        <motion.li
          key={user.id}
          initial={{ opacity: 0, y: 6 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ ...SPRING, delay: Math.min(n, 12) * 0.025 }}
          className="flex items-center gap-2.5 rounded-xl px-2 py-1.5"
        >
          <UserAvatar user={user} className="size-8" />
          <span className="min-w-0 flex-1 truncate text-sm font-bold">{displayName(user)}</span>
        </motion.li>
      ))}
      {more && (
        <li>
          <button type="button" onClick={loadMore} className="flex w-full items-center justify-center gap-1 rounded-xl py-2 text-xs font-bold text-primary hover:bg-primary/10">
            <PlusIcon className="size-3.5" />
            {t("chattools.poll.showMore")}
          </button>
        </li>
      )}
    </ul>
  );
}

/** A moderator's "Remove all 👍", asked once more in place. */
function ClearOne({ reaction, onClear }: { reaction: ReactionLike; onClear: (reaction: ReactionLike) => Promise<void> }) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="mt-3 flex items-center justify-end gap-2 border-t pt-3">
      {error && <span className="mr-auto text-xs text-destructive first-letter:uppercase">{error}</span>}
      <motion.button
        type="button"
        whileTap={{ scale: 0.95 }}
        disabled={busy}
        onClick={() => {
          setBusy(true);
          setError(null);
          onClear(reaction)
            .catch((err: Error) => setError(err.message))
            .finally(() => setBusy(false));
        }}
        className="flex items-center gap-1.5 rounded-xl px-3 py-1.5 text-xs font-bold text-destructive transition-colors hover:bg-destructive/10 disabled:opacity-60"
      >
        {busy ? <LoaderCircleIcon className="size-3.5 animate-spin" /> : <Trash2Icon className="size-3.5" />}
        {t("chattools.reactions.clearOne", { emoji: emojiLabel(reaction) })}
      </motion.button>
    </div>
  );
}
