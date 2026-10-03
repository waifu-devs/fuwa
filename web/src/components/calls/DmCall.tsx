import { LockKeyholeIcon, PhoneCallIcon, PhoneIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { Conversation } from "@/gen/fuwa/v1/dm_pb";
import type { User } from "@/gen/fuwa/v1/types_pb";
import { hangUp, joinCall } from "@/calls/engine";
import { useCalls, useInDmCall } from "@/calls/state";
import { useFuwa } from "@/fuwa/store";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { displayName } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { cn } from "@/lib/utils";
import { clock } from "./CallPanel";
import { HangUpButton, MuteButtons, ParticipantMenu, useSpeaking, VoiceAvatar } from "./parts";

/** The call going on in a conversation, if there is one. */
export const useDmCall = (instanceKey: string, conversationId: string) => useFuwa((s) => s.instances[instanceKey]?.dms.calls[conversationId]);

/** Start a call (or join the one going on) from a conversation's header. */
export function CallButton({ instanceKey, conversationId }: { instanceKey: string; conversationId: string }) {
  const call = useDmCall(instanceKey, conversationId);
  const inCall = useInDmCall(instanceKey, conversationId);
  if (inCall) return null;
  const going = !!call?.participants.length;
  return (
    <motion.button
      type="button"
      whileTap={{ scale: 0.85 }}
      onClick={() => void joinCall({ kind: "dm", instance: instanceKey, conversationId })}
      aria-label={going ? "Join the call" : "Start a call"}
      title={going ? "Join the call" : "Start a call (end-to-end encrypted)"}
      className={cn(
        "group grid size-9 place-items-center rounded-full transition-colors",
        going ? "bg-[#3ba55d] text-white hover:brightness-110" : "text-muted-foreground hover:bg-muted hover:text-foreground",
      )}
    >
      {going ? <PhoneCallIcon className="ringing size-[18px]" /> : <PhoneIcon className="size-[18px] transition-transform duration-300 group-hover:-rotate-12 group-hover:scale-110" />}
    </motion.button>
  );
}

/**
 * The call, across the top of the conversation: both of you, glowing while
 * you talk, with the controls. While the other person hasn't picked up,
 * waves go out from them.
 */
export function DmCallStrip({ instanceKey, conversation, me }: { instanceKey: string; conversation: Conversation; me: User }) {
  const call = useDmCall(instanceKey, conversation.id);
  const inCall = useInDmCall(instanceKey, conversation.id);
  const status = useCalls((s) => (inCall ? s.call?.status : null));
  const since = useCalls((s) => (inCall ? s.call?.since : null));
  const now = useNow(1_000);
  const show = inCall || !!call?.participants.length;
  const here = new Set((call?.participants ?? []).map((p) => p.userId));
  if (inCall) here.add(me.id);
  const partner = conversation.users.find((u) => u.id !== me.id);
  const partnerIn = !!partner && here.has(partner.id);
  const startedBy = conversation.users.find((u) => u.id === call?.startedBy);

  let line: string;
  if (!inCall) line = `${displayName(startedBy ?? partner)} started a call`;
  else if (status !== "connected") line = status === "reconnecting" ? "Reconnecting…" : "Connecting…";
  else if (!partnerIn) line = `Calling ${displayName(partner)}…`;
  else line = since ? clock(Math.max(0, Math.floor((now - since) / 1000))) : "In call";

  return (
    <AnimatePresence initial={false}>
      {show && (
        <motion.div
          key="strip"
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: "auto", opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={SPRING}
          className="shrink-0 overflow-hidden border-b bg-[linear-gradient(180deg,color-mix(in_srgb,#3ba55d_14%,var(--background)),var(--background))]"
        >
          <div className="flex flex-col items-center gap-3 px-4 py-5">
            <div className="flex items-center gap-6 sm:gap-10">
              {conversation.users.map((u) => (
                <Person key={u.id} instanceKey={instanceKey} user={u} here={here.has(u.id)} ringing={inCall && !here.has(u.id)} />
              ))}
            </div>
            <p className="flex items-center gap-1.5 text-sm font-bold text-muted-foreground tabular-nums">
              <LockKeyholeIcon className="size-3.5" aria-label="End-to-end encrypted" />
              {line}
            </p>
            {inCall ? (
              <div className="flex items-center gap-2">
                <MuteButtons size="lg" />
                <HangUpButton size="lg" label="Hang up" onClick={() => void hangUp(null)} />
              </div>
            ) : (
              <Button
                onClick={() => void joinCall({ kind: "dm", instance: instanceKey, conversationId: conversation.id })}
                className="group h-11 rounded-2xl bg-[#3ba55d] px-5 font-extrabold text-white hover:bg-[#3ba55d] hover:brightness-110"
              >
                <PhoneIcon className="transition-transform group-hover:-rotate-12" /> Join call
              </Button>
            )}
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function Person({ instanceKey, user, here, ringing }: { instanceKey: string; user: User; here: boolean; ringing: boolean }) {
  const speaking = useSpeaking(user.id);
  return (
    <ParticipantMenu instanceKey={instanceKey} user={user}>
      <button type="button" className="group relative grid place-items-center rounded-full" aria-label={displayName(user)}>
        {ringing && (
          <>
            <span aria-hidden className="call-wave absolute inset-0 rounded-full border-2 border-[#3ba55d]/60" />
            <span aria-hidden className="call-wave absolute inset-0 rounded-full border-2 border-[#3ba55d]/40 [animation-delay:1s]" />
          </>
        )}
        <motion.span animate={{ opacity: here ? 1 : 0.45, scale: speaking ? 1.06 : 1 }} transition={{ type: "spring", stiffness: 400, damping: 16 }}>
          <VoiceAvatar user={user} speaking={speaking} ring={4} className="size-16 text-xl transition-transform group-hover:scale-105 sm:size-20" />
        </motion.span>
      </button>
    </ParticipantMenu>
  );
}
