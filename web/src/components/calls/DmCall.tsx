import { LockKeyholeIcon, PhoneCallIcon, PhoneIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import type { Conversation } from "@/gen/fuwa/v1/dm_pb";
import type { User } from "@/gen/fuwa/v1/types_pb";
import { hangUp, joinCall } from "@/calls/engine";
import { useCalls, useInDmCall } from "@/calls/state";
import { useFuwa } from "@/fuwa/store";
import { SPRING } from "@/lib/motion";
import { Button } from "@/components/ui/button";
import { displayName } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { cn } from "@/lib/utils";
import { useI18n } from "@/i18n/react";
import { clock } from "./call-clock";
import { HangUpButton, MuteButtons, ParticipantMenu, useSpeaking, VoiceAvatar } from "./parts";
import { CameraButton, LiveBadge, PopOutButton, RecordButton, ScreenButton, ScreenSoundButton, TileMedia } from "./Video";

/** The call going on in a conversation, if there is one. */
export const useDmCall = (instanceKey: string, conversationId: string) => useFuwa((s) => s.instances[instanceKey]?.dms.calls[conversationId]);

/** Start a call (or join the one going on) from a conversation's header. */
export function CallButton({ instanceKey, conversationId }: { instanceKey: string; conversationId: string }) {
  const call = useDmCall(instanceKey, conversationId);
  const inCall = useInDmCall(instanceKey, conversationId);
  const { t } = useI18n();
  if (inCall) return null;
  const going = !!call?.participants.length;
  return (
    <motion.button
      type="button"
      whileTap={{ scale: 0.85 }}
      onClick={() => void joinCall({ kind: "dm", instance: instanceKey, conversationId })}
      aria-label={going ? t("dms-calls.calls.dm.join") : t("dms-calls.calls.dm.start")}
      title={going ? t("dms-calls.calls.dm.join") : t("dms-calls.calls.dm.startTitle")}
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
  const { t } = useI18n();
  const { show, inCall, here, filming, sharing } = useStripPeople(instanceKey, conversation, me);
  const line = useStripLine(instanceKey, conversation, me, here);

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
            <Stage instanceKey={instanceKey} users={conversation.users} me={me} inCall={inCall} here={here} filming={filming} sharing={sharing} />
            <p className="flex items-center gap-1.5 text-sm font-bold text-muted-foreground tabular-nums">
              <LockKeyholeIcon className="size-3.5" aria-label={t("dms-calls.dm.encrypted")} />
              {line}
            </p>
            {inCall ? (
              <div className="flex items-center gap-2">
                <MuteButtons size="lg" />
                <CameraButton size="lg" />
                <ScreenButton size="lg" />
                <RecordButton size="lg" />
                <HangUpButton size="lg" label={t("dms-calls.calls.dm.hangUp")} onClick={() => void hangUp(null)} />
              </div>
            ) : (
              <Button
                onClick={() => void joinCall({ kind: "dm", instance: instanceKey, conversationId: conversation.id })}
                className="group h-11 rounded-2xl bg-[#3ba55d] px-5 font-extrabold text-white hover:bg-[#3ba55d] hover:brightness-110"
              >
                <PhoneIcon className="transition-transform group-hover:-rotate-12" /> {t("dms-calls.calls.dm.joinCall")}
              </Button>
            )}
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/** Who's in the call, and whose camera and screen are on (cameras come through only while you're in the call). */
function useStripPeople(instanceKey: string, conversation: Conversation, me: User) {
  const call = useDmCall(instanceKey, conversation.id);
  const inCall = useInDmCall(instanceKey, conversation.id);
  const mine = useCalls((s) => s.selfVideo);
  const myStream = useCalls((s) => s.selfStream);
  const participants = call?.participants ?? [];
  const here = new Set(participants.map((p) => p.userId));
  if (inCall) here.add(me.id);
  const others = inCall ? participants.filter((p) => p.userId !== me.id) : [];
  const filming = new Set(others.filter((p) => p.selfVideo).map((p) => p.userId));
  if (inCall && mine) filming.add(me.id);
  const sharing = new Set(others.filter((p) => p.selfStream).map((p) => p.userId));
  if (inCall && myStream) sharing.add(me.id);
  return { show: inCall || participants.length > 0, inCall, here, filming, sharing };
}

/** The line under the call: who started it, connecting, calling them, or how long it's run; and who's recording. */
function useStripLine(instanceKey: string, conversation: Conversation, me: User, here: Set<string>): string {
  const { t } = useI18n();
  const call = useDmCall(instanceKey, conversation.id);
  const inCall = useInDmCall(instanceKey, conversation.id);
  const status = useCalls((s) => (inCall ? s.call?.status : null));
  const since = useCalls((s) => (inCall ? s.call?.since : null));
  const myRecord = useCalls((s) => s.selfRecord);
  const now = useNow(1_000);
  const partner = conversation.users.find((u) => u.id !== me.id);
  let line: string;
  if (!inCall) line = t("dms-calls.calls.dm.started", { name: displayName(conversation.users.find((u) => u.id === call?.startedBy) ?? partner) });
  else if (status !== "connected") line = t(status === "reconnecting" ? "dms-calls.calls.status.reconnecting" : "dms-calls.calls.status.connecting");
  else if (!partner || !here.has(partner.id)) line = t("dms-calls.calls.dm.calling", { name: displayName(partner) });
  else line = since ? clock(Math.max(0, Math.floor((now - since) / 1000))) : t("dms-calls.calls.dm.inCall");
  const recorders = conversation.users.filter((u) => (u.id === me.id ? inCall && myRecord : call?.participants.some((p) => p.userId === u.id && p.selfRecord)));
  if (!recorders.length) return line;
  return t("dms-calls.calls.dm.recording", { line, names: recorders.map((u) => (u.id === me.id ? t("dms-calls.calls.dm.you") : displayName(u))).join(", ") });
}

/** Tiles while a camera or screen is on, else both of you as big avatars (waves going out from whoever hasn't picked up). */
function Stage({
  instanceKey,
  users,
  me,
  inCall,
  here,
  filming,
  sharing,
}: {
  instanceKey: string;
  users: User[];
  me: User;
  inCall: boolean;
  here: Set<string>;
  filming: Set<string>;
  sharing: Set<string>;
}) {
  return (
    <AnimatePresence mode="popLayout" initial={false}>
      {filming.size || sharing.size ? (
        <motion.div
          key="cameras"
          initial={{ opacity: 0, scale: 0.96 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.96 }}
          transition={SPRING}
          className="grid w-full max-w-3xl grid-cols-1 gap-3 sm:grid-cols-2"
        >
          {users
            .filter((u) => sharing.has(u.id))
            .map((u) => (
              <Screen key={`screen-${u.id}`} instanceKey={instanceKey} user={u} self={u.id === me.id} />
            ))}
          {users.map((u) => (
            <Camera key={u.id} instanceKey={instanceKey} user={u} self={u.id === me.id} here={here.has(u.id)} videoOn={filming.has(u.id)} />
          ))}
        </motion.div>
      ) : (
        <motion.div key="people" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="flex items-center gap-6 sm:gap-10">
          {users.map((u) => (
            <Person key={u.id} instanceKey={instanceKey} user={u} here={here.has(u.id)} ringing={inCall && !here.has(u.id)} />
          ))}
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

/** One of you in a call where a camera is on: a tile, with your camera or your avatar. */
function Camera({ instanceKey, user, self, here, videoOn }: { instanceKey: string; user: User; self: boolean; here: boolean; videoOn: boolean }) {
  const speaking = useSpeaking(user.id);
  const name = displayName(user);
  return (
    <div className={cn("group/tile relative transition-opacity", !here && "opacity-50")}>
      <ParticipantMenu instanceKey={instanceKey} user={user}>
        <button
          type="button"
          aria-label={name}
          className={cn(
            "group relative block aspect-video w-full overflow-hidden rounded-2xl border bg-card shadow-sm transition-[box-shadow,border-color] duration-300",
            speaking ? "border-[#3ba55d] shadow-[0_0_0_2px_#3ba55d,0_10px_40px_-10px_rgb(59_165_93/0.6)]" : "hover:border-primary/40",
          )}
        >
          <TileMedia userId={user.id} user={user} videoOn={videoOn} self={self} speaking={speaking} avatarClass="size-14 text-lg sm:size-16 sm:text-xl" />
          <span className="absolute bottom-2 left-2 max-w-[80%] truncate rounded-lg bg-background/75 px-2 py-0.5 text-xs font-bold backdrop-blur">{name}</span>
        </button>
      </ParticipantMenu>
      {here && <PopOutButton popped={{ instance: instanceKey, userId: user.id }} name={name} className="absolute top-2 right-2" />}
    </div>
  );
}

/** A shared screen in the call: the whole width, shown whole. */
function Screen({ instanceKey, user, self }: { instanceKey: string; user: User; self: boolean }) {
  const name = displayName(user);
  const { t } = useI18n();
  return (
    <motion.div layout initial={{ opacity: 0, scale: 0.95 }} animate={{ opacity: 1, scale: 1 }} transition={SPRING} className="group/tile relative sm:col-span-2">
      <div className="relative aspect-video w-full overflow-hidden rounded-2xl border bg-black shadow-lg">
        <TileMedia userId={user.id} user={user} videoOn self={self} screen speaking={false} avatarClass="size-14 text-lg sm:size-16 sm:text-xl" />
        <span className="absolute bottom-2 left-2 flex max-w-[80%] items-center gap-1.5 rounded-lg bg-background/80 px-2 py-0.5 backdrop-blur">
          <LiveBadge />
          <span className="truncate text-xs font-bold">{self ? t("dms-calls.calls.screen.yours") : t("dms-calls.calls.screen.theirs", { name })}</span>
        </span>
      </div>
      <PopOutButton popped={{ instance: instanceKey, userId: user.id, screen: true }} name={name} className="absolute top-2 right-2" />
      <ScreenSoundButton userId={user.id} self={self} className="absolute top-2 left-2" />
    </motion.div>
  );
}
