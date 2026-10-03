import { useNavigate } from "@tanstack/react-router";
import { timestampMs } from "@bufbuild/protobuf/wkt";
import { LockKeyholeIcon, PhoneIcon, PhoneOffIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo } from "react";
import { joinCall } from "@/calls/engine";
import { declineKey, setCalls, useCalls } from "@/calls/state";
import { useFuwa, type FuwaState } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { displayName } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { play } from "@/lib/sounds";

/** How long a new call rings for. After that it's still there to join from the conversation. */
const RING_MS = 45_000;
const RING_EVERY_MS = 2_600;

type Ringing = { instance: string; conversationId: string; callerId: string; startedAt: number; key: string };

let lastInstances: FuwaState["instances"] | null = null;
let lastList: Ringing[] = [];

/** Calls that started a moment ago in conversations you're in, that you're not in yet. */
function ringing(s: FuwaState): Ringing[] {
  if (s.instances === lastInstances) return lastList;
  lastInstances = s.instances;
  const out: Ringing[] = [];
  for (const inst of Object.values(s.instances)) {
    const me = inst.me?.id;
    if (!me) continue;
    for (const call of Object.values(inst.dms.calls)) {
      if (!call.participants.length || call.participants.some((p) => p.userId === me)) continue;
      const startedAt = call.startedAt ? timestampMs(call.startedAt) : 0;
      out.push({
        instance: inst.key,
        conversationId: call.conversationId,
        callerId: call.startedBy,
        startedAt,
        key: declineKey(inst.key, call.conversationId, startedAt),
      });
    }
  }
  const same = out.length === lastList.length && out.every((r, n) => r.key === lastList[n]!.key);
  if (!same) lastList = out;
  return lastList;
}

/**
 * Someone calling you: a card that slides in wherever you are in the app,
 * with their face, a ring that keeps going, and answer or decline.
 */
export function IncomingCalls() {
  const all = useFuwa(ringing);
  const declined = useCalls((s) => s.declined);
  const current = useCalls((s) => s.call);
  const now = useNow(1_000);
  const shown = useMemo(
    () =>
      all.filter(
        (r) =>
          !declined[r.key] &&
          now - r.startedAt < RING_MS &&
          !(current?.target.kind === "dm" && current.target.instance === r.instance && current.target.conversationId === r.conversationId),
      ),
    [all, declined, now, current],
  );

  const ringingNow = shown.length > 0;
  useEffect(() => {
    if (!ringingNow) return;
    play("ring");
    const id = setInterval(() => play("ring"), RING_EVERY_MS);
    return () => clearInterval(id);
  }, [ringingNow]);

  return (
    <div className="pointer-events-none fixed inset-x-0 top-3 z-[60] flex flex-col items-center gap-2 px-3 sm:inset-x-auto sm:top-auto sm:right-4 sm:bottom-4 sm:items-end">
      <AnimatePresence>
        {shown.map((r) => (
          <CallCard key={r.key} call={r} />
        ))}
      </AnimatePresence>
    </div>
  );
}

function CallCard({ call }: { call: Ringing }) {
  const navigate = useNavigate();
  const caller = useFuwa((s) => s.instances[call.instance]?.dms.conversations.find((c) => c.id === call.conversationId)?.users.find((u) => u.id === call.callerId));
  const where = useFuwa((s) => (s.order.length > 1 ? s.instances[call.instance]?.node?.name : undefined));
  const decline = () => setCalls((s) => ({ declined: { ...s.declined, [call.key]: true } }));
  const answer = () => {
    void joinCall({ kind: "dm", instance: call.instance, conversationId: call.conversationId });
    void navigate({ to: "/$instance/dm/$conversation", params: { instance: call.instance, conversation: call.conversationId } });
  };
  return (
    <motion.div
      layout
      role="alertdialog"
      aria-label={`${displayName(caller)} is calling`}
      initial={{ opacity: 0, y: -24, scale: 0.9 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.9, transition: { duration: 0.18 } }}
      transition={{ type: "spring", stiffness: 420, damping: 26 }}
      className="pointer-events-auto flex w-full max-w-sm items-center gap-3 rounded-3xl border bg-card/95 p-3 pr-3.5 shadow-2xl backdrop-blur"
    >
      <span className="relative grid shrink-0 place-items-center">
        <span aria-hidden className="call-wave absolute inset-0 rounded-full border-2 border-[#3ba55d]/60" />
        <span aria-hidden className="call-wave absolute inset-0 rounded-full border-2 border-[#3ba55d]/40 [animation-delay:1s]" />
        <UserAvatar user={caller} className="size-12 text-base" />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate font-extrabold">{displayName(caller)}</span>
        <span className="flex items-center gap-1 truncate text-xs text-muted-foreground">
          <LockKeyholeIcon className="size-3 shrink-0" /> Encrypted call{where ? ` on ${where}` : ""}
        </span>
      </span>
      <motion.button
        type="button"
        whileTap={{ scale: 0.85 }}
        onClick={decline}
        aria-label="Decline"
        title="Decline"
        className="group grid size-11 shrink-0 place-items-center rounded-full bg-destructive text-white transition hover:brightness-110"
      >
        <PhoneOffIcon className="size-5 transition-transform group-hover:rotate-[135deg]" />
      </motion.button>
      <motion.button
        type="button"
        whileTap={{ scale: 0.85 }}
        onClick={answer}
        aria-label="Answer"
        title="Answer"
        className="grid size-11 shrink-0 place-items-center rounded-full bg-[#3ba55d] text-white transition hover:brightness-110"
      >
        <PhoneIcon className="ringing size-5" />
      </motion.button>
    </motion.div>
  );
}
