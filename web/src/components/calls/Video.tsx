import { CropIcon, ExpandIcon, PictureInPicture2Icon, SparklesIcon, TagIcon, VideoIcon, VideoOffIcon, XIcon } from "lucide-react";
import { motion } from "motion/react";
import { useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Permission, type User, type VoiceState } from "@/gen/fuwa/v1/types_pb";
import { toggleCamera } from "@/calls/engine";
import { getCalls, subscribeCalls, useCalls, type CallTarget } from "@/calls/state";
import { useLayerFor, useVideoTrack } from "@/calls/video";
import { useAccess } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { hue } from "@/components/Icons";
import { displayName, memberName } from "@/lib/format";
import { hasIn } from "@/lib/permissions";
import { setPrefs, usePrefs } from "@/lib/prefs";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { useSpeaking, VoiceAvatar } from "./parts";

/**
 * Cameras in calls: a <video> per camera, the camera button, and popping a
 * camera out into a window of its own, which streaming apps (OBS's Window
 * Capture, for one) pick up as a clean feed of that one person.
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

/** Someone's camera while it's on and coming in (yours with `self`), or null. */
export function useCamera(userId: string | undefined, on: boolean, self: boolean) {
  const track = useVideoTrack(userId, self);
  return on ? track : null;
}

/**
 * What a call tile shows: someone's camera when it's on, or their avatar
 * on their color. The camera comes in the size that fits the tile.
 */
export function TileMedia({
  userId,
  user,
  videoOn,
  self,
  speaking,
  avatarClass = "size-16 text-xl sm:size-20 sm:text-2xl",
  fit = "cover",
  children,
}: {
  userId: string;
  user: User | undefined;
  videoOn: boolean;
  self: boolean;
  speaking: boolean;
  avatarClass?: string;
  fit?: "cover" | "contain";
  children?: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const track = useCamera(userId, videoOn, self);
  const mirror = usePrefs((p) => p.mirrorVideo);
  useLayerFor(userId, ref, !!track && !self);
  return (
    <div ref={ref} className="absolute inset-0 grid place-items-center">
      <span aria-hidden className="server-gradient absolute inset-0 opacity-25 transition-opacity duration-500 group-hover:opacity-35" style={hue(userId)} />
      {track ? (
        <motion.div key="video" initial={{ opacity: 0, scale: 1.04 }} animate={{ opacity: 1, scale: 1 }} transition={{ duration: 0.35, ease: [0.22, 1, 0.36, 1] }} className="absolute inset-0 bg-black">
          <VideoView track={track} fit={fit} mirror={self && mirror} />
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
export function CameraButton({ size = "sm" }: { size?: "sm" | "lg" }) {
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

// ───────────────────────── Popped-out cameras ─────────────────────────

type Popped = { instance: string; userId: string; serverId?: string };

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
  if (popped.some((x) => x.instance === p.instance && x.userId === p.userId)) {
    windows.get(keyOf(p))?.focus();
    return;
  }
  setPopped([...popped, p]);
}

const keyOf = (p: Popped) => `${p.instance}/${p.userId}`;
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
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        popOut(p);
      }}
      aria-label={`Pop out ${name}`}
      title={`Pop out ${name} (a window of their own, for streaming apps too)`}
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
    const w = window.open("", `fuwa-camera-${p.userId}`, "popup,width=640,height=360");
    if (!w) {
      toast("Your browser blocked the pop-out window. Allow pop-ups for fuwa, then try again.");
      setPopped(popped.filter((x) => keyOf(x) !== key));
      return;
    }
    w.document.body.replaceChildren();
    dress(w.document, `${nameRef.current} · fuwa camera`);
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
    if (w) w.document.title = `${name} · fuwa camera`;
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
  const mine = useCalls((s) => s.selfVideo);
  const videoOn = self ? mine : !!state?.selfVideo;
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
      <TileMedia userId={p.userId} user={user} videoOn={videoOn} self={self} speaking={speaking && glow} fit={fit} avatarClass="size-28 text-4xl">
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
        <FeedToggle
          on={fit === "cover"}
          onClick={() => setPrefs({ popoutFit: fit === "cover" ? "contain" : "cover" })}
          label={fit === "cover" ? "Fit the whole picture" : "Fill the window"}
          icon={fit === "cover" ? CropIcon : ExpandIcon}
        />
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
