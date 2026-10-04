import { CheckIcon, CircleDotIcon, CropIcon, LaptopIcon, ServerIcon, ExpandIcon, MonitorIcon, MonitorUpIcon, MonitorXIcon, PictureInPicture2Icon, SparklesIcon, TagIcon, VideoIcon, VideoOffIcon, Volume2Icon, VolumeXIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Permission, type User, type VoiceState } from "@/gen/fuwa/v1/types_pb";
import { setRecording, setScreenSound, setServerRecording, shareScreen, toggleCamera, toggleRecording, toggleScreen, toggleScreenQuiet } from "@/calls/engine";
import { getCalls, subscribeCalls, useCalls, type CallTarget } from "@/calls/state";
import { canShareScreen, canShareSound, feedOf, useLayerFor, useVideoTrack } from "@/calls/video";
import { useAccess } from "@/fuwa/hooks";
import { store, useFuwa } from "@/fuwa/store";
import { hue } from "@/components/Icons";
import { displayName, memberName } from "@/lib/format";
import { hasIn } from "@/lib/permissions";
import { setPrefs, usePrefs } from "@/lib/prefs";
import { cue } from "@/lib/sounds";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { useSpeaking, VoiceAvatar } from "./parts";

/**
 * Cameras and shared screens in calls: a <video> per feed, the camera and
 * screen buttons, and popping a feed out into a window of its own, which
 * streaming apps (OBS's Window Capture, for one) pick up as a clean feed
 * of that one person, or their screen.
 */

/** A camera track, playing. Fades in once its first frame is there. */
export function VideoView({ track, mirror, fit = "cover", className }: { track: MediaStreamTrack; mirror?: boolean; fit?: "cover" | "contain"; className?: string }) {
  const ref = useRef<HTMLVideoElement>(null);
  const [ready, setReady] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    setReady(false);
    el.srcObject = new MediaStream([track]);
    void el.play().catch(() => {});
    return () => {
      el.srcObject = null;
    };
  }, [track]);
  return (
    <video
      ref={ref}
      autoPlay
      playsInline
      muted
      onLoadedData={() => setReady(true)}
      className={cn(
        "size-full transition-opacity duration-300",
        fit === "cover" ? "object-cover" : "object-contain",
        mirror && "-scale-x-100",
        ready ? "opacity-100" : "opacity-0",
        className,
      )}
    />
  );
}

/** Someone's camera (or shared screen) while it's on and coming in (yours with `self`), or null. */
export function useCamera(userId: string | undefined, on: boolean, self: boolean, screen = false) {
  const track = useVideoTrack(userId && feedOf(userId, screen), self);
  return on ? track : null;
}

/**
 * What a call tile shows: someone's camera when it's on, or their avatar
 * on their color. The camera comes in the size that fits the tile. With
 * `screen`, their shared screen instead, whole and never mirrored.
 */
export function TileMedia({
  userId,
  user,
  videoOn,
  self,
  speaking,
  screen = false,
  avatarClass = "size-16 text-xl sm:size-20 sm:text-2xl",
  fit = "cover",
  children,
}: {
  userId: string;
  user: User | undefined;
  videoOn: boolean;
  self: boolean;
  speaking: boolean;
  screen?: boolean;
  avatarClass?: string;
  fit?: "cover" | "contain";
  children?: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const track = useCamera(userId, videoOn, self, screen);
  const mirror = usePrefs((p) => p.mirrorVideo);
  useLayerFor(feedOf(userId, screen), ref, !!track && !self);
  return (
    <div ref={ref} className="absolute inset-0 grid place-items-center">
      <span aria-hidden className="server-gradient absolute inset-0 opacity-25 transition-opacity duration-500 group-hover:opacity-35" style={hue(userId)} />
      {track ? (
        <motion.div key="video" initial={{ opacity: 0, scale: 1.04 }} animate={{ opacity: 1, scale: 1 }} transition={{ duration: 0.35, ease: [0.22, 1, 0.36, 1] }} className="absolute inset-0 bg-black">
          <VideoView track={track} fit={screen ? "contain" : fit} mirror={self && mirror && !screen} />
        </motion.div>
      ) : (
        <motion.span key="avatar" initial={{ opacity: 0, scale: 0.8 }} animate={{ opacity: 1, scale: speaking ? 1.06 : 1 }} transition={{ type: "spring", stiffness: 400, damping: 15 }} className="relative">
          <VoiceAvatar user={user} speaking={speaking} ring={4} className={avatarClass} />
        </motion.span>
      )}
      {children}
    </div>
  );
}

/** Whether you may turn your camera on in the call you're in: VIDEO in a voice channel, always in a conversation. */
export function useMayFilm(target: CallTarget | null | undefined): boolean {
  const serverId = target?.kind === "voice" ? target.serverId : "";
  const access = useAccess(target?.instance ?? "", serverId);
  if (!target) return false;
  if (target.kind === "dm") return true;
  return hasIn(access, target.channelId, Permission.VIDEO);
}

/** Turns your camera on and off in the call you're in. */
export function CameraButton({ size = "sm", className }: { size?: "sm" | "lg"; className?: string }) {
  const on = useCalls((s) => s.selfVideo);
  const target = useCalls((s) => s.call?.target);
  const may = useMayFilm(target);
  const label = !may ? "You can't turn your camera on here" : on ? "Turn camera off" : "Turn camera on";
  return (
    <button
      type="button"
      onClick={() => void toggleCamera()}
      disabled={!may}
      aria-pressed={on}
      aria-label={label}
      title={label}
      className={cn(
        "group relative grid shrink-0 place-items-center transition active:scale-90 disabled:pointer-events-none disabled:opacity-40",
        size === "sm" ? "size-8 rounded-lg" : "size-12 rounded-2xl",
        on ? "bg-[#3ba55d] text-white hover:brightness-110" : size === "sm" ? "text-muted-foreground hover:bg-muted hover:text-foreground" : "bg-muted text-foreground hover:bg-muted/70",
        className,
      )}
    >
      <motion.span key={String(on)} initial={{ scale: 0.5, y: 4 }} animate={{ scale: 1, y: 0 }} transition={{ type: "spring", stiffness: 600, damping: 14 }} className="grid place-items-center">
        {on ? (
          <VideoIcon className={cn(size === "sm" ? "size-[18px]" : "size-5", "transition-transform group-hover:scale-110")} />
        ) : (
          <VideoOffIcon className={cn(size === "sm" ? "size-[18px]" : "size-5", "transition-transform group-hover:scale-110")} />
        )}
      </motion.span>
    </button>
  );
}

/**
 * Shares your screen in the call you're in, or stops. Not on phones, which
 * can't. Where the instance passes a screen's sound on, starting opens a
 * menu: with its sound, or the picture only (the choice is remembered, and
 * the keyboard shortcut uses it). Browsers that can't share sound say so there.
 */
export function ScreenButton({ size = "sm", className }: { size?: "sm" | "lg"; className?: string }) {
  const on = useCalls((s) => s.selfStream);
  const target = useCalls((s) => s.call?.target);
  const offered = useCalls((s) => s.screenSoundOffered);
  const withSound = usePrefs((p) => p.shareSound);
  const may = useMayFilm(target);
  if (!canShareScreen()) return null;
  const label = !may ? "You can't share your screen here" : on ? "Stop sharing your screen" : "Share your screen";
  const Icon = on ? MonitorXIcon : MonitorUpIcon;
  const menu = offered && may && !on;
  const button = (
    <button
      type="button"
      onClick={menu ? undefined : () => void toggleScreen()}
      disabled={!may}
      aria-pressed={on}
      aria-label={label}
      title={label}
      className={cn(
        "group relative grid shrink-0 place-items-center transition active:scale-90 disabled:pointer-events-none disabled:opacity-40",
        size === "sm" ? "size-8 rounded-lg" : "size-12 rounded-2xl",
        on ? "bg-primary text-primary-foreground hover:brightness-110" : size === "sm" ? "text-muted-foreground hover:bg-muted hover:text-foreground" : "bg-muted text-foreground hover:bg-muted/70",
        className,
      )}
    >
      <motion.span key={String(on)} initial={{ scale: 0.5, y: on ? 6 : -4 }} animate={{ scale: 1, y: 0 }} transition={{ type: "spring", stiffness: 600, damping: 14 }} className="grid place-items-center">
        <Icon className={cn(size === "sm" ? "size-[18px]" : "size-5", "transition-transform", on ? "group-hover:scale-110" : "group-hover:-translate-y-0.5")} />
      </motion.span>
    </button>
  );
  if (!menu) return button;
  const sound = canShareSound();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>{button}</DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="center" className="w-72">
        <DropdownMenuLabel className="text-xs text-muted-foreground">Share your screen</DropdownMenuLabel>
        <DropdownMenuItem disabled={!sound} onSelect={() => void shareScreen(true)} className="items-start gap-2.5 py-2">
          <Volume2Icon className="mt-0.5" />
          <span className="min-w-0">
            <span className="block font-bold">With its sound</span>
            <span className="block text-xs text-muted-foreground">
              {sound ? "A tab's sound, or the whole screen's where your system allows. Your call's sound stays out." : "This browser shares the picture only. Chrome and Edge can share sound."}
            </span>
          </span>
          {sound && withSound && <CheckIcon className="mt-0.5 ml-auto text-primary" />}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void shareScreen(false)} className="items-start gap-2.5 py-2">
          <MonitorIcon className="mt-0.5" />
          <span className="min-w-0">
            <span className="block font-bold">Picture only</span>
            <span className="block text-xs text-muted-foreground">No sound goes with it.</span>
          </span>
          {(!sound || !withSound) && <CheckIcon className="mt-0.5 ml-auto text-primary" />}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * The sound button on a shared screen: yours turns its sound off for
 * everyone (the share goes on); someone else's turns it off for you. Its
 * bars dance while the screen plays something. Only there while sound comes.
 */
export function ScreenSoundButton({ userId, self, className }: { userId: string; self: boolean; className?: string }) {
  const mine = useCalls((s) => s.screenSound);
  const coming = useCalls((s) => !!s.screenSounds[userId]);
  const quiet = useCalls((s) => !!s.quietScreens[userId]);
  const playing = useSpeaking(feedOf(userId, true));
  const shown = self ? mine !== null : coming;
  const on = self ? mine === true : !quiet;
  const label = self ? (on ? "Stop sharing your screen's sound" : "Share your screen's sound again") : on ? "Turn this screen's sound off for you" : "Turn this screen's sound back on";
  return (
    <AnimatePresence>
      {shown && (
        <motion.button
          type="button"
          initial={{ opacity: 0, scale: 0.6 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.6 }}
          transition={{ type: "spring", stiffness: 600, damping: 22 }}
          onClick={(e) => {
            e.stopPropagation();
            if (self) setScreenSound(!on);
            else toggleScreenQuiet(userId);
          }}
          aria-pressed={!on}
          aria-label={label}
          title={label}
          className={cn(
            "flex h-8 items-center gap-1 rounded-xl px-2 shadow-sm backdrop-blur transition hover:scale-105 active:scale-90",
            on ? "bg-background/75 text-foreground hover:bg-background" : "bg-[#ed4245] text-white",
            className,
          )}
        >
          <motion.span key={String(on)} initial={{ scale: 0.4, rotate: -20 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 600, damping: 14 }} className="grid place-items-center">
            {on ? <Volume2Icon className="size-4" /> : <VolumeXIcon className="size-4" />}
          </motion.span>
          {on && <SoundBars playing={playing} />}
        </motion.button>
      )}
    </AnimatePresence>
  );
}

/** Three little bars that bounce while something plays, and rest when it doesn't. */
function SoundBars({ playing }: { playing: boolean }) {
  return (
    <span aria-hidden className="flex h-3 items-end gap-[2px]">
      {[0, 1, 2].map((i) => (
        <span
          key={i}
          className={cn("w-[3px] origin-bottom rounded-full bg-current transition-transform duration-300", playing ? "animate-[sound-bar_0.9s_ease-in-out_infinite]" : "scale-y-[0.3]")}
          style={{ height: "100%", animationDelay: `${i * 0.15}s` }}
        />
      ))}
    </span>
  );
}

/** Whether you may record the call you're in: RECORD in a voice channel, always in a conversation. */
export function useMayRecord(target: CallTarget | null | undefined): boolean {
  const serverId = target?.kind === "voice" ? target.serverId : "";
  const access = useAccess(target?.instance ?? "", serverId);
  if (!target) return false;
  if (target.kind === "dm") return true;
  return hasIn(access, target.channelId, Permission.RECORD);
}

/**
 * Records the call: its sound to a file on this device, or (in a voice
 * channel, where the instance allows it) everyone's sound on the server, a
 * track per person. With both to choose from it opens a menu. Hidden where
 * you can't record.
 */
export function RecordButton({ size = "sm", className }: { size?: "sm" | "lg"; className?: string }) {
  const device = useCalls((s) => s.selfRecord);
  const server = useCalls((s) => s.serverRecord);
  const offered = useCalls((s) => s.serverRecordings);
  const target = useCalls((s) => s.call?.target);
  const may = useMayRecord(target);
  const video = useFuwa((s) => target?.kind === "voice" && !!s.instances[target.instance]?.servers.find((x) => x.id === target.serverId)?.recordVideo);
  const on = device || server;
  if (!may && !on) return null;
  const both = target?.kind === "voice" && (offered || server);
  const label = !both ? (on ? "Stop recording and save it" : "Record the call's sound (everyone sees you are)") : on ? "Recording: stop it here" : "Record this channel";
  const button = (
    <button
      type="button"
      onClick={both ? undefined : toggleRecording}
      aria-pressed={on}
      aria-label={label}
      title={label}
      className={cn(
        "group relative grid shrink-0 place-items-center transition active:scale-90",
        size === "sm" ? "size-8 rounded-lg" : "size-12 rounded-2xl",
        on ? "bg-[#ed4245] text-white hover:brightness-110" : size === "sm" ? "text-muted-foreground hover:bg-muted hover:text-foreground" : "bg-muted text-foreground hover:bg-muted/70",
        className,
      )}
    >
      {on && <span aria-hidden className="absolute inset-0 animate-ping rounded-[inherit] bg-[#ed4245]/40 [animation-duration:2s]" />}
      <motion.span key={String(on)} initial={{ scale: 0.4 }} animate={{ scale: 1 }} transition={{ type: "spring", stiffness: 600, damping: 14 }} className="relative grid place-items-center">
        <CircleDotIcon className={cn(size === "sm" ? "size-[18px]" : "size-5", "transition-transform group-hover:scale-110")} />
      </motion.span>
    </button>
  );
  if (!both) return button;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>{button}</DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="center" className="w-72">
        <DropdownMenuLabel className="text-xs text-muted-foreground">Everyone in the channel sees it, and hears a beep</DropdownMenuLabel>
        <DropdownMenuItem onSelect={() => setRecording(!device)} className="items-start gap-2.5 py-2">
          <LaptopIcon className="mt-0.5" />
          <span className="min-w-0">
            <span className="block font-bold">{device ? "Stop and save the file" : "On this device"}</span>
            <span className="block text-xs text-muted-foreground">{device ? "Your recording downloads now." : "The call's sound as you hear it, saved here as one file."}</span>
          </span>
          {device && <RecordingDot />}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => setServerRecording(!server)} className="items-start gap-2.5 py-2">
          <ServerIcon className="mt-0.5" />
          <span className="min-w-0">
            <span className="block font-bold">{server ? "Stop recording on the server" : "On the server"}</span>
            <span className="block text-xs text-muted-foreground">
              {server ? "It stays in this channel's recordings." : video ? "Everyone's sound, camera and screen, kept for people who can record here." : "A track per person, kept for people who can record here."}
            </span>
          </span>
          {server && <RecordingDot />}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function RecordingDot() {
  return (
    <span aria-hidden className="relative mt-1.5 ml-auto grid size-2 shrink-0 place-items-center">
      <span className="absolute inset-0 animate-ping rounded-full bg-[#ed4245]/60" />
      <span className="size-2 rounded-full bg-[#ed4245]" />
    </span>
  );
}

/**
 * Says so, with a sound, when someone else in your call starts recording
 * it. Mounted once; reads only who's recording, so it wakes for nothing else.
 */
export function RecordingWatch() {
  const target = useCalls((s) => s.call?.target);
  const recording = useFuwa((s) => {
    const inst = target && s.instances[target.instance];
    if (!inst || !target) return "";
    const states = target.kind === "voice" ? inst.voice[target.serverId]?.filter((v) => v.channelId === target.channelId) : inst.dms.calls[target.conversationId]?.participants;
    return (states ?? [])
      .flatMap((v) => (v.userId === inst.me?.id ? [] : [...(v.selfRecord ? [v.userId] : []), ...(v.serverRecord ? [`server:${v.userId}`] : [])]))
      .sort()
      .join(" ");
  });
  const seen = useRef<Set<string>>(new Set());
  useEffect(() => {
    const now = new Set(recording ? recording.split(" ") : []);
    const fresh = [...now].filter((id) => !seen.current.has(id));
    seen.current = now;
    if (!fresh.length || !target) return;
    const inst = store.get().instances[target.instance];
    const name = (ids: string[]) => ids.map((id) => displayName(inst?.users[id])).join(", ");
    const onServer = fresh.filter((id) => id.startsWith("server:")).map((id) => id.slice(7));
    const onDevice = fresh.filter((id) => !id.startsWith("server:"));
    cue("recording");
    if (onDevice.length) toast(`${name(onDevice)} started recording this call.`);
    const video = target.kind === "voice" && !!inst?.servers.find((x) => x.id === target.serverId)?.recordVideo;
    if (onServer.length) toast(`${name(onServer)} started recording this channel on the server${video ? ", with cameras and shared screens" : ""}.`);
  }, [recording, target]);
  return null;
}

/** The little "LIVE" mark on a shared screen, breathing while it's live. */
export function LiveBadge({ className }: { className?: string }) {
  return (
    <span className={cn("relative inline-flex shrink-0 items-center gap-1 rounded-md bg-[#ed4245] px-1.5 py-px text-[10px] font-extrabold tracking-wider text-white", className)}>
      <span aria-hidden className="relative grid size-1.5 place-items-center">
        <span className="absolute inset-0 animate-ping rounded-full bg-white/70" />
        <span className="size-1.5 rounded-full bg-white" />
      </span>
      LIVE
    </span>
  );
}

// ───────────────────────── Popped-out cameras ─────────────────────────

/** A popped-out feed: someone's camera, or with `screen` their shared screen. */
type Popped = { instance: string; userId: string; serverId?: string; screen?: boolean };

let popped: Popped[] = [];
const poppedListeners = new Set<() => void>();
const setPopped = (next: Popped[]) => {
  popped = next;
  for (const l of poppedListeners) l();
};
const usePopped = () =>
  useSyncExternalStore(
    (l) => {
      poppedListeners.add(l);
      return () => void poppedListeners.delete(l);
    },
    () => popped,
  );

/** Opens someone's camera (or avatar, while it's off) in a window of its own. */
export function popOut(p: Popped) {
  if (popped.some((x) => keyOf(x) === keyOf(p))) {
    windows.get(keyOf(p))?.focus();
    return;
  }
  setPopped([...popped, p]);
}

const keyOf = (p: Popped) => `${p.instance}/${feedOf(p.userId, p.screen)}`;
/** The window's title, which streaming apps show when picking a window. */
const titleOf = (p: Popped, name: string) => (p.screen ? `${name}'s screen · fuwa` : `${name} · fuwa camera`);
const windows = new Map<string, Window>();

// Hanging up closes every popped-out camera.
if (typeof window !== "undefined") {
  subscribeCalls(() => {
    if (!getCalls().call && popped.length) setPopped([]);
  });
  window.addEventListener("pagehide", () => {
    for (const w of windows.values()) w.close();
  });
}

/** The pop-out button on a tile. */
export function PopOutButton({ popped: p, name, className }: { popped: Popped; name: string; className?: string }) {
  const what = p.screen ? `${name}'s screen` : name;
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        popOut(p);
      }}
      aria-label={`Pop out ${what}`}
      title={`Pop out ${what} (a window of its own, for streaming apps too)`}
      className={cn(
        "grid size-8 place-items-center rounded-xl bg-background/75 text-foreground opacity-0 shadow-sm backdrop-blur transition hover:scale-105 hover:bg-background active:scale-90 group-hover/tile:opacity-100 focus-visible:opacity-100 max-sm:opacity-100",
        className,
      )}
    >
      <PictureInPicture2Icon className="size-4" />
    </button>
  );
}

/** Every popped-out camera's window. Mounted once, beside the call panel. */
export function PopOuts() {
  const list = usePopped();
  return (
    <>
      {list.map((p) => (
        <PopOutWindow key={keyOf(p)} popped={p} />
      ))}
    </>
  );
}

/** Copies the app's styles and theme into a new window, so the portal inside looks the same. */
function dress(doc: Document, title: string) {
  doc.title = title;
  for (const node of document.head.querySelectorAll('style, link[rel="stylesheet"]')) doc.head.appendChild(node.cloneNode(true));
  const root = document.documentElement;
  for (const { name, value } of [...root.attributes]) doc.documentElement.setAttribute(name, value);
  doc.body.className = "m-0 h-screen overflow-hidden bg-black";
  const meta = doc.createElement("meta");
  meta.name = "color-scheme";
  meta.content = "dark";
  doc.head.appendChild(meta);
}

function PopOutWindow({ popped: p }: { popped: Popped }) {
  const user = useFuwa((s) => s.instances[p.instance]?.users[p.userId]);
  const member = useFuwa((s) => (p.serverId ? s.instances[p.instance]?.members[p.serverId]?.find((m) => m.user?.id === p.userId) : undefined));
  const name = member ? memberName(member) : displayName(user);
  const [container, setContainer] = useState<HTMLElement | null>(null);
  const nameRef = useRef(name);
  nameRef.current = name;

  useEffect(() => {
    const key = keyOf(p);
    // Named per person, so streaming apps find the same window again next time.
    const w = window.open("", `fuwa-${p.screen ? "screen" : "camera"}-${p.userId}`, p.screen ? "popup,width=960,height=540" : "popup,width=640,height=360");
    if (!w) {
      toast("Your browser blocked the pop-out window. Allow pop-ups for fuwa, then try again.");
      setPopped(popped.filter((x) => keyOf(x) !== key));
      return;
    }
    w.document.body.replaceChildren();
    dress(w.document, titleOf(p, nameRef.current));
    const div = w.document.createElement("div");
    div.className = "h-full";
    w.document.body.appendChild(div);
    windows.set(key, w);
    setContainer(div);
    const closed = () => setPopped(popped.filter((x) => keyOf(x) !== key));
    w.addEventListener("pagehide", closed);
    return () => {
      w.removeEventListener("pagehide", closed);
      windows.delete(key);
      w.close();
    };
  }, [p]);

  useEffect(() => {
    const w = windows.get(keyOf(p));
    if (w) w.document.title = titleOf(p, name);
  }, [name, p]);

  return container ? createPortal(<PopOutFeed popped={p} user={user} name={name} />, container) : null;
}

/** Whoever's call state this is, in the call you're in. */
function useCallState(p: Popped): VoiceState | undefined {
  const target = useCalls((s) => s.call?.target);
  return useFuwa((s) => {
    const inst = s.instances[p.instance];
    if (!inst || !target) return undefined;
    if (target.kind === "voice") return inst.voice[target.serverId]?.find((v) => v.userId === p.userId);
    return inst.dms.calls[target.conversationId]?.participants.find((v) => v.userId === p.userId);
  });
}

/**
 * Inside a popped-out window: the camera edge to edge, nothing else, so
 * it's a clean feed. Moving the mouse shows a few choices for a moment:
 * the name, a glow while they talk, and filling or fitting the window.
 */
function PopOutFeed({ popped: p, user, name }: { popped: Popped; user: User | undefined; name: string }) {
  const me = useFuwa((s) => s.instances[p.instance]?.me?.id);
  const self = p.userId === me;
  const state = useCallState(p);
  const mine = useCalls((s) => (p.screen ? s.selfStream : s.selfVideo));
  const videoOn = self ? mine : !!(p.screen ? state?.selfStream : state?.selfVideo);
  const speaking = useSpeaking(p.userId);
  const showName = usePrefs((x) => x.popoutName);
  const glow = usePrefs((x) => x.popoutGlow);
  const fit = usePrefs((x) => x.popoutFit);
  const [awake, setAwake] = useState(true);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const wake = () => {
    setAwake(true);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setAwake(false), 1_600);
  };
  useEffect(() => {
    timer.current = setTimeout(() => setAwake(false), 2_400);
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  return (
    <div onPointerMove={wake} className={cn("group/feed relative h-full w-full select-none overflow-hidden bg-black text-white", !awake && "cursor-none")}>
      <TileMedia userId={p.userId} user={user} videoOn={videoOn} self={self} screen={p.screen} speaking={speaking && glow} fit={fit} avatarClass="size-28 text-4xl">
        <span
          aria-hidden
          className={cn("pointer-events-none absolute inset-0 transition-[box-shadow] duration-300", glow && speaking ? "shadow-[inset_0_0_0_4px_#3ba55d,inset_0_0_40px_rgb(59_165_93/0.45)]" : "shadow-none")}
        />
      </TileMedia>
      {showName && (
        <span className="absolute bottom-3 left-3 max-w-[70%] truncate rounded-xl bg-black/60 px-3 py-1 text-sm font-bold backdrop-blur">{name}</span>
      )}
      <div className={cn("absolute top-3 right-3 flex gap-1.5 transition-opacity duration-300", awake ? "opacity-100" : "pointer-events-none opacity-0")}>
        <FeedToggle on={showName} onClick={() => setPrefs({ popoutName: !showName })} label={showName ? "Hide the name" : "Show the name"} icon={TagIcon} />
        <FeedToggle on={glow} onClick={() => setPrefs({ popoutGlow: !glow })} label={glow ? "No glow while talking" : "Glow while talking"} icon={SparklesIcon} />
        {/* A screen always shows whole: cropping it would cut off what's being shown. */}
        {!p.screen && (
          <FeedToggle
            on={fit === "cover"}
            onClick={() => setPrefs({ popoutFit: fit === "cover" ? "contain" : "cover" })}
            label={fit === "cover" ? "Fit the whole picture" : "Fill the window"}
            icon={fit === "cover" ? CropIcon : ExpandIcon}
          />
        )}
        <FeedToggle on={false} onClick={() => setPopped(popped.filter((x) => keyOf(x) !== keyOf(p)))} label="Close" icon={XIcon} />
      </div>
    </div>
  );
}

function FeedToggle({ on, onClick, label, icon: Icon }: { on: boolean; onClick: () => void; label: string; icon: typeof TagIcon }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={on}
      aria-label={label}
      title={label}
      className={cn("grid size-9 place-items-center rounded-xl backdrop-blur transition hover:scale-105 active:scale-90", on ? "bg-white/25 text-white" : "bg-black/50 text-white/70 hover:text-white")}
    >
      <Icon className="size-4" />
    </button>
  );
}
