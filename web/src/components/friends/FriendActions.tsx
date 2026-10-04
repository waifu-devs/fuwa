import { BanIcon, CheckIcon, ClockIcon, LoaderCircleIcon, ShieldOffIcon, UserCheckIcon, UserMinusIcon, UserPlusIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import type { GetRelationshipResponse } from "@/gen/fuwa/v1/friend_pb";
import type { User } from "@/gen/fuwa/v1/types_pb";
import { run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { acceptFriend, blockUser, getRelationship, removeFriend, sendFriendRequest, unblockUser } from "@/fuwa/friends";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { displayName } from "@/lib/format";
import { BLOCKED, FRIEND, INCOMING, OUTGOING, stateWith } from "@/lib/friends";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * Where you stand with someone, on their profile card: add them, take or
 * turn down their request, cancel yours, unfriend, block. Mutual friends
 * show when everyone involved allows it. Your list (kept live) decides the
 * buttons; the instance is asked only for what the list can't say.
 */
export function FriendActions({ instanceKey, user }: { instanceKey: string; user: User }) {
  const state = useFuwa((s) => stateWith(s.instances[instanceKey]?.friends.list ?? [], user.id));
  const [relation, setRelation] = useState<GetRelationshipResponse | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    run(getRelationship(instanceKey, user.id)).then(
      (r) => live && setRelation(r),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [instanceKey, user.id, state]);

  async function act(action: () => Promise<unknown>, done?: string) {
    if (busy) return;
    setBusy(true);
    try {
      await action();
      if (done) toast(done);
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setBusy(false);
    }
  }
  const name = displayName(user);
  const mutual = relation?.mutualFriends ?? [];
  const mayRequest = relation?.mayRequest ?? true;

  return (
    <div className="flex flex-col gap-1.5">
      <AnimatePresence initial={false}>
        {mutual.length > 0 && (
          <motion.p
            key="mutual"
            initial={{ opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={SPRING}
            className="flex items-center gap-2 px-1.5 pt-0.5 text-xs text-muted-foreground"
          >
            <span className="flex -space-x-1.5">
              {mutual.slice(0, 3).map((u, n) => (
                <motion.span key={u.id} initial={{ scale: 0 }} animate={{ scale: 1 }} transition={{ ...SPRING, delay: n * 0.05 }} className="rounded-full ring-2 ring-popover">
                  <UserAvatar user={u} className="size-5 text-[0.5rem]" />
                </motion.span>
              ))}
            </span>
            <span className="truncate">
              {mutual.length === 1 ? `${displayName(mutual[0])} is a friend of you both` : `${mutual.length} mutual friends`}
            </span>
          </motion.p>
        )}
      </AnimatePresence>
      <div className="flex gap-1.5">
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.div
            key={state}
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -6 }}
            transition={SPRING}
            className="flex min-w-0 flex-1 gap-1.5"
          >
            {state === 0 && (
              <Action
                icon={busy ? <LoaderCircleIcon className="animate-spin" /> : <UserPlusIcon />}
                label="Add friend"
                disabled={busy || !mayRequest}
                title={mayRequest ? undefined : `${name} isn't taking friend requests`}
                onClick={() => void act(() => run(sendFriendRequest(instanceKey, { userId: user.id })), `Friend request sent to ${name}`)}
              />
            )}
            {state === OUTGOING && (
              <Action icon={<ClockIcon />} label="Requested" hoverLabel="Cancel request" tone="bad" disabled={busy} onClick={() => void act(() => run(removeFriend(instanceKey, user.id)))} />
            )}
            {state === INCOMING && (
              <>
                <Action icon={<CheckIcon />} label="Accept" tone="good" disabled={busy} onClick={() => void act(() => run(acceptFriend(instanceKey, user.id)), `You and ${name} are friends`)} />
                <Action icon={<XIcon />} label="Decline" disabled={busy} onClick={() => void act(() => run(removeFriend(instanceKey, user.id)))} />
              </>
            )}
            {state === FRIEND && (
              <Action icon={<UserCheckIcon />} label="Friends" hoverLabel="Remove friend" hoverIcon={<UserMinusIcon />} tone="bad" disabled={busy} onClick={() => void act(() => run(removeFriend(instanceKey, user.id)), `Removed ${name} from your friends`)} />
            )}
            {state === BLOCKED && (
              <Action icon={<ShieldOffIcon />} label="Unblock" disabled={busy} onClick={() => void act(() => run(unblockUser(instanceKey, user.id)), `Unblocked ${name}`)} />
            )}
          </motion.div>
        </AnimatePresence>
        {state !== BLOCKED && (
          <motion.button
            type="button"
            whileTap={{ scale: 0.9 }}
            disabled={busy}
            onClick={() => void act(() => run(blockUser(instanceKey, user.id)), `Blocked ${name}. They won't be told.`)}
            aria-label={`Block ${name}`}
            title={`Block ${name}: they can't message you or send requests, and aren't told`}
            className="group grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition hover:bg-destructive/10 hover:text-destructive disabled:opacity-60"
          >
            <BanIcon className="size-4 transition-transform duration-300 group-hover:-rotate-45" />
          </motion.button>
        )}
      </div>
    </div>
  );
}

function Action({
  icon,
  label,
  hoverLabel,
  hoverIcon,
  tone,
  disabled,
  title,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  /** What a click does, when it isn't what the button says (Friends: remove). */
  hoverLabel?: string;
  hoverIcon?: React.ReactNode;
  tone?: "good" | "bad";
  disabled?: boolean;
  title?: string;
  onClick: () => void;
}) {
  return (
    <motion.button
      type="button"
      whileTap={{ scale: 0.96 }}
      disabled={disabled}
      title={title ?? hoverLabel}
      aria-label={hoverLabel ?? label}
      onClick={onClick}
      className={cn(
        "group relative flex h-9 min-w-0 flex-1 items-center justify-center gap-1.5 overflow-hidden rounded-xl px-3 text-sm font-bold transition-colors disabled:opacity-60 [&_svg]:size-4",
        tone === "good"
          ? "bg-emerald-500/15 text-emerald-700 hover:bg-emerald-500/25 dark:text-emerald-300"
          : hoverLabel && tone === "bad"
            ? "bg-muted text-foreground hover:bg-destructive/10 hover:text-destructive"
            : "bg-muted text-foreground hover:bg-muted/70",
      )}
    >
      {hoverLabel ? (
        <>
          <span className="flex items-center gap-1.5 transition duration-200 group-hover:-translate-y-6 group-hover:opacity-0">
            {icon}
            <span className="truncate">{label}</span>
          </span>
          <span className="absolute inset-0 flex translate-y-6 items-center justify-center gap-1.5 opacity-0 transition duration-200 group-hover:translate-y-0 group-hover:opacity-100">
            {hoverIcon ?? <XIcon />}
            <span className="truncate">{hoverLabel}</span>
          </span>
        </>
      ) : (
        <>
          {icon}
          <span className="truncate">{label}</span>
        </>
      )}
    </motion.button>
  );
}
