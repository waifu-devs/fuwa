import { AnimatePresence, motion } from "motion/react";
import { useMemo } from "react";
import type { VoiceState } from "@/gen/fuwa/v1/types_pb";
import { useFuwa } from "@/fuwa/store";
import { displayName, memberName } from "@/lib/format";
import { cn } from "@/lib/utils";
import { ParticipantMenu, useSpeaking, VoiceAvatar, VoiceFlags } from "./parts";

const NONE: VoiceState[] = [];

/** Who's in a voice channel, in the order they joined. */
export function useVoiceIn(instanceKey: string, serverId: string, channelId: string): VoiceState[] {
  const all = useFuwa((s) => s.instances[instanceKey]?.voice[serverId] ?? NONE);
  return useMemo(() => {
    const here = all.filter((v) => v.channelId === channelId);
    return here.length ? here : NONE;
  }, [all, channelId]);
}

/** The people in a voice channel, listed under it in the sidebar, each popping in as they join. */
export function VoiceUsers({ instanceKey, serverId, channelId }: { instanceKey: string; serverId: string; channelId: string }) {
  const states = useVoiceIn(instanceKey, serverId, channelId);
  return (
    <AnimatePresence initial={false}>
      {states.length > 0 && (
        <motion.ul
          key="list"
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: "auto", opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={{ duration: 0.22, ease: [0.22, 1, 0.36, 1] }}
          className="ml-6 flex flex-col overflow-hidden"
        >
          <AnimatePresence initial={false}>
            {states.map((state) => (
              <VoiceUserRow key={state.userId} instanceKey={instanceKey} serverId={serverId} channelId={channelId} state={state} />
            ))}
          </AnimatePresence>
        </motion.ul>
      )}
    </AnimatePresence>
  );
}

function VoiceUserRow({ instanceKey, serverId, channelId, state }: { instanceKey: string; serverId: string; channelId: string; state: VoiceState }) {
  const user = useFuwa((s) => s.instances[instanceKey]?.users[state.userId]);
  const member = useFuwa((s) => s.instances[instanceKey]?.members[serverId]?.find((m) => m.user?.id === state.userId));
  const speaking = useSpeaking(state.userId);
  const name = member ? memberName(member) : displayName(user);
  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, x: -12, scale: 0.9 }}
      animate={{ opacity: 1, x: 0, scale: 1 }}
      exit={{ opacity: 0, x: -12, scale: 0.9, transition: { duration: 0.16 } }}
      transition={{ type: "spring", stiffness: 520, damping: 30 }}
    >
      <ParticipantMenu instanceKey={instanceKey} serverId={serverId} channelId={channelId} user={user} state={state}>
        <button
          type="button"
          className={cn(
            "group flex w-full items-center gap-2 rounded-lg px-2 py-1 text-left text-sm transition-colors hover:bg-muted/70",
            speaking ? "text-foreground" : "text-muted-foreground hover:text-foreground",
          )}
        >
          <VoiceAvatar user={user} speaking={speaking} ring={2} className="size-6 text-[0.6rem]" />
          <span className={cn("min-w-0 flex-1 truncate transition-[font-weight]", speaking && "font-bold")}>
            {name}
          </span>
          <VoiceFlags state={state} />
        </button>
      </ParticipantMenu>
    </motion.li>
  );
}
