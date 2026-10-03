import { ChevronLeftIcon, HeadphonesIcon, MicOffIcon, Volume2Icon } from "lucide-react";
import { AnimatePresence, LayoutGroup, motion } from "motion/react";
import { useEffect } from "react";
import { Permission, type Channel, type VoiceState } from "@/gen/fuwa/v1/types_pb";
import { hangUp, joinCall } from "@/calls/engine";
import { useCalls, useInVoice } from "@/calls/state";
import { useAccess, useInstance } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { CopyId } from "@/components/CopyId";
import { SPRING, SwapText } from "@/components/motion";
import { useLayout } from "@/components/Shell";
import { Button } from "@/components/ui/button";
import { AppBadge } from "@/components/AppBadge";
import { displayName, isAgent, memberName } from "@/lib/format";
import { hasIn } from "@/lib/permissions";
import { setTitle } from "@/lib/notify";
import { cn } from "@/lib/utils";
import { HangUpButton, MuteButtons, ParticipantMenu, useSpeaking, VoiceFlags } from "./parts";
import { CameraButton, LiveBadge, PopOutButton, RecordButton, ScreenButton, TileMedia } from "./Video";
import { RecordingsButton } from "./Recordings";
import { useVoiceIn } from "./VoiceUsers";

/**
 * A voice channel, open: shared screens on top, big, then everyone in it as
 * a tile that glows while they talk (their camera, when it's on), and the
 * controls for your own place there. Each tile is one person's own stream,
 * and pops out into a window of its own.
 */
export function VoiceStage({ instanceKey, serverId, channel }: { instanceKey: string; serverId: string; channel: Channel }) {
  const inst = useInstance(instanceKey);
  const access = useAccess(instanceKey, serverId);
  const states = useVoiceIn(instanceKey, serverId, channel.id);
  const joined = useInVoice(instanceKey, channel.id);
  const status = useCalls((s) => (joined ? s.call?.status : null));
  const { compact, setNavOpen } = useLayout();
  const canConnect = hasIn(access, channel.id, Permission.CONNECT);
  const canSpeak = hasIn(access, channel.id, Permission.SPEAK);
  const serverName = inst?.servers.find((s) => s.id === serverId)?.name;
  const me = inst?.me?.id;
  const myStream = useCalls((s) => s.selfStream);
  // Screens come through only while you're in the channel; yours from this browser.
  const sharing = joined ? states.filter((s) => (s.userId === me ? myStream : s.selfStream)) : [];

  useEffect(() => {
    setTitle(`🔊 ${channel.name} · ${serverName ?? "fuwa"}`);
    return () => setTitle("fuwa");
  }, [channel.name, serverName]);

  const join = () => void joinCall({ kind: "voice", instance: instanceKey, serverId, channelId: channel.id });

  return (
    <div className="flex h-full min-h-0 flex-col bg-[radial-gradient(ellipse_at_top,color-mix(in_srgb,var(--primary)_10%,transparent),transparent_65%)]">
      <header className="flex h-14 shrink-0 items-center gap-2 border-b px-2 sm:px-4">
        {compact && (
          <button
            type="button"
            aria-label="Channels"
            onClick={() => setNavOpen(true)}
            className="grid size-9 place-items-center rounded-full text-muted-foreground transition hover:-translate-x-0.5 hover:bg-muted"
          >
            <ChevronLeftIcon className="size-5" />
          </button>
        )}
        <Volume2Icon className="size-5 shrink-0 text-muted-foreground" />
        <h1 className="truncate font-extrabold">
          <SwapText className="truncate align-bottom">{channel.name}</SwapText>
        </h1>
        <CopyId id={channel.id} what="channel ID" />
        <span className="flex-1" />
        <RecordingPill instanceKey={instanceKey} serverId={serverId} states={states} />
        <RecordingsButton instanceKey={instanceKey} serverId={serverId} channel={channel} states={states} />
        <AnimatePresence>
          {states.length > 0 && (
            <motion.span
              initial={{ opacity: 0, scale: 0.8 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.8 }}
              transition={SPRING}
              className="rounded-full bg-muted px-2.5 py-1 text-xs font-bold text-muted-foreground tabular-nums"
            >
              {states.length} in voice
            </motion.span>
          )}
        </AnimatePresence>
      </header>

      <div className="scroll-thin flex min-h-0 flex-1 flex-col overflow-y-auto p-4 sm:p-6">
        {states.length === 0 ? (
          <Empty name={channel.name} />
        ) : (
          <LayoutGroup>
            <AnimatePresence initial={false}>
              {sharing.length > 0 && (
                <motion.ul
                  key="screens"
                  layout
                  initial={{ opacity: 0, height: 0 }}
                  animate={{ opacity: 1, height: "auto" }}
                  exit={{ opacity: 0, height: 0 }}
                  transition={SPRING}
                  className={cn("mx-auto mb-4 grid w-full max-w-5xl shrink-0 gap-3 sm:gap-4", sharing.length > 1 && "lg:grid-cols-2")}
                >
                  <AnimatePresence initial={false} mode="popLayout">
                    {sharing.map((state) => (
                      <ScreenTile key={state.userId} instanceKey={instanceKey} serverId={serverId} state={state} self={state.userId === me} />
                    ))}
                  </AnimatePresence>
                </motion.ul>
              )}
            </AnimatePresence>
            <motion.ul layout className={cn("m-auto grid w-full max-w-5xl gap-3 sm:gap-4", sharing.length > 0 && "mt-0", gridFor(states.length))}>
              <AnimatePresence initial={false} mode="popLayout">
                {states.map((state, n) => (
                  <Tile key={state.userId} instanceKey={instanceKey} serverId={serverId} channelId={channel.id} state={state} index={n} />
                ))}
              </AnimatePresence>
            </motion.ul>
          </LayoutGroup>
        )}
      </div>

      <footer className="flex shrink-0 flex-col items-center gap-2 border-t bg-card/60 px-4 py-3 backdrop-blur">
        <AnimatePresence mode="wait" initial={false}>
          {joined ? (
            <motion.div key="in" initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 12 }} transition={SPRING} className="flex items-center gap-2">
              <MuteButtons size="lg" />
              <CameraButton size="lg" />
              <ScreenButton size="lg" />
              <RecordButton size="lg" />
              <HangUpButton size="lg" onClick={() => void hangUp(null)} />
            </motion.div>
          ) : (
            <motion.div key="out" initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 12 }} transition={SPRING} className="flex flex-col items-center gap-1.5">
              <Button onClick={join} disabled={!canConnect} className="btn group h-12 rounded-2xl px-6 font-extrabold">
                <HeadphonesIcon className="transition-transform group-hover:-rotate-12 group-hover:scale-110" /> Join voice
              </Button>
              {!canConnect ? (
                <p className="text-xs text-muted-foreground">You can't join this channel.</p>
              ) : (
                !canSpeak && (
                  <p className="flex items-center gap-1 text-xs text-muted-foreground">
                    <MicOffIcon className="size-3" /> You can listen here, but not speak.
                  </p>
                )
              )}
            </motion.div>
          )}
        </AnimatePresence>
        <AnimatePresence>
          {joined && status !== "connected" && (
            <motion.p initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="text-xs font-bold text-muted-foreground">
              {status === "reconnecting" ? "Reconnecting…" : "Connecting…"}
            </motion.p>
          )}
        </AnimatePresence>
      </footer>
    </div>
  );
}

/** Columns for this many people: one big tile, a pair, then a grid. */
function gridFor(n: number) {
  if (n === 1) return "max-w-xl grid-cols-1";
  if (n === 2) return "max-w-3xl grid-cols-1 sm:grid-cols-2";
  if (n <= 4) return "grid-cols-2";
  if (n <= 9) return "grid-cols-2 md:grid-cols-3";
  return "grid-cols-2 md:grid-cols-3 lg:grid-cols-4";
}

function Tile({ instanceKey, serverId, channelId, state, index }: { instanceKey: string; serverId: string; channelId: string; state: VoiceState; index: number }) {
  const user = useFuwa((s) => s.instances[instanceKey]?.users[state.userId]);
  const member = useFuwa((s) => s.instances[instanceKey]?.members[serverId]?.find((m) => m.user?.id === state.userId));
  const self = useFuwa((s) => s.instances[instanceKey]?.me?.id === state.userId);
  const joined = useInVoice(instanceKey, channelId);
  const mine = useCalls((s) => s.selfVideo);
  const speaking = useSpeaking(state.userId);
  const name = member ? memberName(member) : displayName(user);
  // Your own camera comes from this browser; anyone else's through the call.
  const videoOn = joined && (self ? mine : state.selfVideo);
  return (
    <motion.li
      layout
      initial={{ opacity: 0, scale: 0.8, y: 16 }}
      animate={{ opacity: 1, scale: 1, y: 0, transition: { ...SPRING, delay: Math.min(index, 8) * 0.04 } }}
      exit={{ opacity: 0, scale: 0.85, transition: { duration: 0.18 } }}
      transition={SPRING}
      className="group/tile relative"
    >
      <ParticipantMenu instanceKey={instanceKey} serverId={serverId} channelId={channelId} user={user} state={state}>
        <button
          type="button"
          className={cn(
            "group relative block aspect-video w-full overflow-hidden rounded-3xl border bg-card text-left shadow-sm transition-[box-shadow,border-color] duration-300",
            speaking ? "border-[#3ba55d] shadow-[0_0_0_2px_#3ba55d,0_10px_40px_-10px_rgb(59_165_93/0.6)]" : "hover:border-primary/40",
          )}
        >
          <TileMedia userId={state.userId} user={user} videoOn={videoOn} self={self} speaking={speaking} />
          <span className="absolute inset-x-2 bottom-2 flex items-center gap-1.5 rounded-xl bg-background/75 px-2.5 py-1 backdrop-blur">
            <span className="min-w-0 flex-1 truncate text-sm font-bold">{name}</span>
            {isAgent(member?.user ?? user) && <AppBadge agent />}
            <VoiceFlags state={state} />
          </span>
        </button>
      </ParticipantMenu>
      {joined && <PopOutButton popped={{ instance: instanceKey, userId: state.userId, serverId }} name={name} className="absolute top-2 right-2" />}
    </motion.li>
  );
}

/** Who's recording the channel, for everyone to see, while anyone is: on the server, or on their device. */
function RecordingPill({ instanceKey, serverId, states }: { instanceKey: string; serverId: string; states: VoiceState[] }) {
  const onServer = states.some((v) => v.serverRecord);
  const names = useFuwa((s) =>
    states
      .filter((v) => v.selfRecord || v.serverRecord)
      .map((v) => {
        const member = s.instances[instanceKey]?.members[serverId]?.find((m) => m.user?.id === v.userId);
        return member ? memberName(member) : displayName(s.instances[instanceKey]?.users[v.userId]);
      })
      .join(", "),
  );
  return (
    <AnimatePresence>
      {names && (
        <motion.span
          initial={{ opacity: 0, scale: 0.8, x: 8 }}
          animate={{ opacity: 1, scale: 1, x: 0 }}
          exit={{ opacity: 0, scale: 0.8 }}
          transition={SPRING}
          title={`${names} ${names.includes(",") ? "are" : "is"} recording this channel${onServer ? " (on the server)" : ""}`}
          className="flex max-w-[45%] items-center gap-1.5 rounded-full bg-[#ed4245]/12 px-2.5 py-1 text-xs font-bold text-[#ed4245]"
        >
          <span aria-hidden className="relative grid size-2 place-items-center">
            <span className="absolute inset-0 animate-ping rounded-full bg-[#ed4245]/60" />
            <span className="size-2 rounded-full bg-[#ed4245]" />
          </span>
          <span className="truncate">
            {onServer ? "Recording on the server" : "Recording"} · {names}
          </span>
        </motion.span>
      )}
    </AnimatePresence>
  );
}

/** Someone's shared screen: shown whole, with whose it is and that it's live. */
function ScreenTile({ instanceKey, serverId, state, self }: { instanceKey: string; serverId: string; state: VoiceState; self: boolean }) {
  const user = useFuwa((s) => s.instances[instanceKey]?.users[state.userId]);
  const member = useFuwa((s) => s.instances[instanceKey]?.members[serverId]?.find((m) => m.user?.id === state.userId));
  const name = member ? memberName(member) : displayName(user);
  return (
    <motion.li
      layout
      initial={{ opacity: 0, scale: 0.94, y: -12 }}
      animate={{ opacity: 1, scale: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.94, transition: { duration: 0.18 } }}
      transition={SPRING}
      className="group/tile relative"
    >
      <div className="relative aspect-video w-full overflow-hidden rounded-3xl border bg-black shadow-lg">
        <TileMedia userId={state.userId} user={user} videoOn self={self} screen speaking={false} />
        <span className="absolute bottom-2 left-2 flex max-w-[80%] items-center gap-1.5 rounded-xl bg-background/80 px-2.5 py-1 backdrop-blur">
          <LiveBadge />
          <span className="truncate text-sm font-bold">{self ? "Your screen" : `${name}'s screen`}</span>
        </span>
      </div>
      <PopOutButton popped={{ instance: instanceKey, userId: state.userId, serverId, screen: true }} name={name} className="absolute top-2 right-2" />
    </motion.li>
  );
}

function Empty({ name }: { name: string }) {
  return (
    <div className="m-auto flex flex-col items-center gap-3 text-center">
      <span className="float relative grid size-20 place-items-center rounded-full bg-primary/12 text-primary">
        <span aria-hidden className="call-wave absolute inset-0 rounded-full border-2 border-primary/40" />
        <span aria-hidden className="call-wave absolute inset-0 rounded-full border-2 border-primary/30 [animation-delay:1s]" />
        <Volume2Icon className="size-9" />
      </span>
      <p className="text-lg font-extrabold">Nobody's in {name} yet</p>
      <p className="max-w-xs text-sm text-muted-foreground">Join, and whoever comes by can talk with you.</p>
    </div>
  );
}
