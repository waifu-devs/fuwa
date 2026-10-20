import { Link } from "@tanstack/react-router";
import { LockKeyholeIcon, PhoneCallIcon, ShieldAlertIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { memo, useEffect, useMemo, useRef } from "react";
import type { Conversation } from "@/gen/fuwa/v1/dm_pb";
import { isMessage, lineText, type Item } from "@/e2ee/vault";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { PartnerDot } from "@/components/Presence";
import { useLayout } from "@/components/Shell";
import { Count } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { useContextMenu } from "@/components/ContextMenu";
import { useVirtualList } from "@/components/virtual";
import { dmMenu } from "@/components/menus/dm";
import { displayName } from "@/lib/format";
import { blockedIds } from "@/lib/friends";
import { cn } from "@/lib/utils";
import { type I18n, T, useI18n } from "@/i18n/react";
import { EMOJI_TOKEN } from "@/lib/emoji";

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
  const { t } = useI18n();
  // Conversations pile up over time, so only the ones in view are drawn, each at its place; a conversation
  // that moves to the top glides there (`.member-line`).
  const keys = useMemo(() => conversations.map((c) => c.id), [conversations]);
  const { listRef, measure, total, lines } = useVirtualList(keys, { estimate: 44 });
  // Only conversations that just started slide in; rows scrolled into view just appear.
  const known = useRef<Set<string> | null>(null);
  const present = useMemo(() => {
    const ids = new Set(keys);
    const fresh = known.current ? new Set(keys.filter((id) => !known.current!.has(id))) : new Set<string>();
    return { ids, fresh };
  }, [keys]);
  useEffect(() => {
    known.current = present.ids;
  }, [present]);
  if (status === "off") return null;
  return (
    <section aria-label={t("dms-calls.dm.list.label")} className="mt-4">
      <p className="mb-1 flex items-center gap-1.5 px-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">
        {t("dms-calls.dm.list.label")}
        <motion.span
          title={t("dms-calls.dm.encrypted")}
          initial={{ scale: 0.4, rotate: -30, opacity: 0 }}
          animate={{ scale: 1, rotate: 0, opacity: 1 }}
          transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.1 }}
          className="inline-grid text-seal-icon"
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
            className="mx-2 my-1 flex items-start gap-2 rounded-xl border border-seal-warn-edge bg-seal-warn-surface px-2.5 py-2 text-xs text-seal-warn-ink"
          >
            <ShieldAlertIcon className="mt-0.5 size-3.5 shrink-0" />
            <span>{problem ?? t("dms-calls.dm.unavailable")}</span>
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
              <span className="grid size-6 place-items-center rounded-lg bg-seal-surface text-seal-ink transition-transform duration-500 group-hover:rotate-[-12deg]">
                <LockKeyholeIcon className="size-3.5" />
              </span>
              {t("dms-calls.dm.list.emptyTitle")}
            </p>
            <p className="mt-1.5 leading-relaxed">
              <T k="dms-calls.dm.list.emptyText" values={{ message: <b className="text-foreground">{t("dms-calls.dm.list.emptyMessage")}</b> }} />
            </p>
          </motion.div>
        )}
      </AnimatePresence>
      <div ref={listRef as React.RefObject<HTMLDivElement | null>} className="relative" style={{ height: total }}>
        <AnimatePresence initial={false} custom={present.ids} presenceAffectsLayout={false}>
          {lines.map((line) => (
            <div key={line.key} ref={measure} data-vkey={line.key} className="member-line absolute inset-x-0 top-0" style={{ transform: `translateY(${line.top}px)` }}>
              <DmLink instanceKey={instanceKey} conversation={conversations[line.index]!} enter={present.fresh.has(line.key)} />
            </div>
          ))}
        </AnimatePresence>
      </div>
    </section>
  );
}

/** What a conversation's last line says, for the list. Read from this browser's copy, never the instance. */
function preview(t: I18n["t"], items: Item[] | undefined, meId: string | undefined): string {
  if (!items) return "";
  for (let n = items.length - 1; n >= 0; n--) {
    const item = items[n]!;
    if (!isMessage(item)) continue;
    if (item.deleted) return t("dms-calls.dm.deleted");
    const text = lineText(item).replace(EMOJI_TOKEN, ":$2:").replace(/[*_~`>#]+/g, "").replace(/\s+/g, " ").trim();
    return item.senderId === meId ? t("dms-calls.dm.list.youSaid", { text }) : text;
  }
  return "";
}

/** One conversation: memoized, so it redraws for its own messages, unread count or call, not the list's every change. */
const DmLink = memo(function DmLink({ instanceKey, conversation, enter }: { instanceKey: string; conversation: Conversation; enter: boolean }) {
  const { compact, setNavOpen } = useLayout();
  const { t } = useI18n();
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id);
  const items = useFuwa((s) => s.instances[instanceKey]?.dms.items[conversation.id]);
  const unread = useFuwa((s) => s.instances[instanceKey]?.dms.unread[conversation.id] ?? 0);
  const calling = useFuwa((s) => !!s.instances[instanceKey]?.dms.calls[conversation.id]?.participants.length);
  const other = conversation.users.find((u) => u.id !== meId) ?? conversation.users[0];
  const line = preview(t, items, meId);
  const menu = useContextMenu("dm", () => dmMenu({ instanceKey, conversation, other }));
  return (
    <motion.div
      initial={enter ? { opacity: 0, x: -10 } : false}
      animate={{ opacity: 1, x: 0 }}
      exit="exit"
      variants={{
        // A conversation that went away slides out; one that only scrolled out of view goes at once.
        exit: (present: Set<string>) =>
          present.has(conversation.id) ? { opacity: 1, transition: { duration: 0 } } : { opacity: 0, x: -10, transition: { duration: 0.15 } },
      }}
      transition={SPRING}
    >
      <Link
        to="/$instance/dm/$conversation"
        params={{ instance: instanceKey, conversation: conversation.id }}
        onClick={() => compact && setNavOpen(false)}
        {...menu}
        className="group flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-sm transition hover:bg-muted data-[menu-open]:bg-muted data-[status=active]:bg-primary/15"
      >
        <span className="relative shrink-0">
          <UserAvatar user={other} className="size-8 text-xs transition-transform duration-300 group-hover:scale-105 group-hover:-rotate-3" />
          <span className="absolute -bottom-0.5 -left-0.5 grid size-3.5 place-items-center rounded-full bg-card text-seal-icon ring-2 ring-card">
            <LockKeyholeIcon className="size-2.5 transition-transform duration-300 group-hover:scale-125" />
          </span>
          <PartnerDot instanceKey={instanceKey} userId={other?.id} className="absolute -right-0.5 -bottom-0.5" />
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
              {line || t("dms-calls.dm.encrypted")}
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
              title={t("dms-calls.dm.list.calling")}
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
});
