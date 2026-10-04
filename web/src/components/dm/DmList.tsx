import { Link } from "@tanstack/react-router";
import { LockKeyholeIcon, PhoneCallIcon, ShieldAlertIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useMemo, type Ref } from "react";
import type { Conversation } from "@/gen/fuwa/v1/dm_pb";
import type { Item } from "@/e2ee/vault";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { useLayout } from "@/components/Shell";
import { Count, SPRING } from "@/components/motion";
import { displayName } from "@/lib/format";
import { blockedIds } from "@/lib/friends";
import { cn } from "@/lib/utils";

const NONE: Conversation[] = [];

/** Your encrypted conversations on an instance, in the instance's sidebar: the latest first. */
export function DmList({ instanceKey }: { instanceKey: string }) {
  const all = useFuwa((s) => s.instances[instanceKey]?.dms.conversations ?? NONE);
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id);
  // Conversations with people you blocked stay out of sight until you unblock them.
  const friends = useFuwa((s) => s.instances[instanceKey]?.friends.list);
  const conversations = useMemo(() => {
    const blocked = blockedIds(friends ?? []);
    if (blocked.size === 0) return all;
    return all.filter((c) => !c.users.some((u) => u.id !== meId && blocked.has(u.id)));
  }, [all, friends, meId]);
  const status = useFuwa((s) => s.instances[instanceKey]?.dms.status ?? "off");
  const problem = useFuwa((s) => s.instances[instanceKey]?.dms.problem ?? null);
  if (status === "off") return null;
  return (
    <section aria-label="Direct messages" className="mt-4">
      <p className="mb-1 flex items-center gap-1.5 px-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">
        Direct messages
        <motion.span
          title="End-to-end encrypted"
          initial={{ scale: 0.4, rotate: -30, opacity: 0 }}
          animate={{ scale: 1, rotate: 0, opacity: 1 }}
          transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.1 }}
          className="inline-grid text-emerald-500"
        >
          <LockKeyholeIcon className="size-3" />
        </motion.span>
      </p>
      <AnimatePresence initial={false} mode="popLayout">
        {(status === "unsupported" || status === "failed") && (
          <motion.p
            key="problem"
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            className="mx-2 my-1 flex items-start gap-2 rounded-xl bg-amber-500/10 px-2.5 py-2 text-xs text-amber-700 dark:text-amber-300"
          >
            <ShieldAlertIcon className="mt-0.5 size-3.5 shrink-0" />
            <span>{problem ?? "Encrypted messages aren't available here."}</span>
          </motion.p>
        )}
        {status !== "unsupported" && status !== "failed" && conversations.length === 0 && (
          <motion.div
            key="empty"
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={SPRING}
            className="group mx-1 mt-1 rounded-2xl border border-dashed px-3 py-3 text-xs text-muted-foreground"
          >
            <p className="flex items-center gap-2 font-bold text-foreground">
              <span className="grid size-6 place-items-center rounded-lg bg-emerald-500/15 text-emerald-600 transition-transform duration-500 group-hover:rotate-[-12deg] dark:text-emerald-400">
                <LockKeyholeIcon className="size-3.5" />
              </span>
              Private by default
            </p>
            <p className="mt-1.5 leading-relaxed">
              Open someone's profile and press <b className="text-foreground">Message</b>. Only the two of you can read what you write.
            </p>
          </motion.div>
        )}
        {conversations.map((c, n) => (
          <DmLink key={c.id} instanceKey={instanceKey} conversation={c} index={n} />
        ))}
      </AnimatePresence>
    </section>
  );
}

/** What a conversation's last line says, for the list. Read from this browser's copy, never the instance. */
function preview(items: Item[] | undefined, meId: string | undefined): string {
  if (!items) return "";
  for (let n = items.length - 1; n >= 0; n--) {
    const item = items[n]!;
    if (item.kind !== "text") continue;
    if (item.deleted) return "Message deleted";
    const text = item.content.replace(/[*_~`>#]+/g, "").replace(/\s+/g, " ").trim();
    return item.senderId === meId ? `You: ${text}` : text;
  }
  return "";
}

function DmLink({
  instanceKey,
  conversation,
  index,
  ref,
}: {
  instanceKey: string;
  conversation: Conversation;
  index: number;
  ref?: Ref<HTMLDivElement>;
}) {
  const { compact, setNavOpen } = useLayout();
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id);
  const items = useFuwa((s) => s.instances[instanceKey]?.dms.items[conversation.id]);
  const unread = useFuwa((s) => s.instances[instanceKey]?.dms.unread[conversation.id] ?? 0);
  const calling = useFuwa((s) => !!s.instances[instanceKey]?.dms.calls[conversation.id]?.participants.length);
  const other = conversation.users.find((u) => u.id !== meId) ?? conversation.users[0];
  const line = preview(items, meId);
  return (
    <motion.div
      ref={ref}
      layout="position"
      initial={{ opacity: 0, x: -10 }}
      animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: Math.min(index, 12) * 0.03 } }}
      exit={{ opacity: 0, x: -10, transition: { duration: 0.15 } }}
      transition={SPRING}
    >
      <Link
        to="/$instance/dm/$conversation"
        params={{ instance: instanceKey, conversation: conversation.id }}
        onClick={() => compact && setNavOpen(false)}
        className="group flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-sm transition hover:bg-muted data-[status=active]:bg-primary/15"
      >
        <span className="relative shrink-0">
          <UserAvatar user={other} className="size-8 text-xs transition-transform duration-300 group-hover:scale-105 group-hover:-rotate-3" />
          <span className="absolute -right-0.5 -bottom-0.5 grid size-3.5 place-items-center rounded-full bg-card text-emerald-500 ring-2 ring-card">
            <LockKeyholeIcon className="size-2.5 transition-transform duration-300 group-hover:scale-125" />
          </span>
        </span>
        <span className="min-w-0 flex-1">
          <span className={cn("block truncate font-bold transition-transform duration-300 group-hover:translate-x-0.5", unread > 0 && "text-foreground")}>
            {displayName(other)}
          </span>
          <AnimatePresence initial={false} mode="popLayout">
            <motion.span
              key={line || "encrypted"}
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={SPRING}
              className={cn("block truncate text-xs", unread > 0 ? "font-bold text-foreground/80" : "text-muted-foreground")}
            >
              {line || "End-to-end encrypted"}
            </motion.span>
          </AnimatePresence>
        </span>
        <AnimatePresence>
          {calling && (
            <motion.span
              key="call"
              initial={{ scale: 0, rotate: -40 }}
              animate={{ scale: 1, rotate: 0 }}
              exit={{ scale: 0, rotate: 40 }}
              transition={{ type: "spring", stiffness: 600, damping: 16 }}
              title="A call is going on"
              className="grid size-6 shrink-0 place-items-center rounded-full bg-[#3ba55d] text-white"
            >
              <PhoneCallIcon className="ringing size-3.5" />
            </motion.span>
          )}
        </AnimatePresence>
        <AnimatePresence>
          {unread > 0 && (
            <motion.span
              initial={{ scale: 0 }}
              animate={{ scale: 1 }}
              exit={{ scale: 0 }}
              transition={{ type: "spring", stiffness: 600, damping: 18 }}
              className="grid h-5 min-w-5 place-items-center rounded-full bg-primary px-1.5 text-[0.7rem] font-extrabold text-primary-foreground"
            >
              <Count value={unread} max={99} />
            </motion.span>
          )}
        </AnimatePresence>
      </Link>
    </motion.div>
  );
}
