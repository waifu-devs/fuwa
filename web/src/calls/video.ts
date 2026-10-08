import { useEffect, useSyncExternalStore, type RefObject } from "react";
import { i18n, type Key } from "@/i18n/i18n";
import { cameraEncodings, type Ceiling, ceilingOf, widthFor } from "@/lib/camera-quality";
import { getPrefs } from "@/lib/prefs";
import { shareBox, wantsMotion, type ShareQuality, type ShareSurface } from "@/lib/screen-share";

/** A sentence in the app's language now. */
const tr = (key: Key) => i18n().t(key);

/**
 * Cameras and shared screens in your call: everyone else's as their tracks
 * arrive, and yours. Each is a feed: a camera's is its person's user id, a
 * shared screen's that and "-screen" (as the media server names its stream).
 * This changes only when a camera comes or goes, never per frame: the
 * pictures themselves go straight from the track into <video> elements.
 *
 * Each place that shows someone's camera says how big it is, and the media
 * server sends the size that fits (see `Layer`): a small tile gets a
 * quarter of the camera, a big tile or a popped-out window gets all of it,
 * and a camera nobody shows isn't sent at all.
 */

/** One of a camera's sizes: full, half, a quarter, or none. */
export type Layer = "h" | "m" | "l" | "off";

export type RemoteVideo = { track: MediaStreamTrack; mid: string };

type Videos = { remote: Record<string, RemoteVideo>; local: MediaStreamTrack | null; localScreen: MediaStreamTrack | null };

let videos: Videos = { remote: {}, local: null, localScreen: null };

const SCREEN = "-screen";

/** The feed of someone's camera, or of their shared screen. */
export const feedOf = (userId: string, screen = false) => (screen ? userId + SCREEN : userId);
/** Whose a feed is. */
export const ownerOf = (feed: string) => (feed.endsWith(SCREEN) ? feed.slice(0, -SCREEN.length) : feed);
export const isScreen = (feed: string) => feed.endsWith(SCREEN);
const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

export const getVideos = () => videos;

export function setRemoteVideo(userId: string, video: RemoteVideo | null) {
  const { [userId]: before, ...rest } = videos.remote;
  if (before === video || (!before && !video)) return;
  videos = { ...videos, remote: video ? { ...rest, [userId]: video } : rest };
  emit();
}

export function clearRemoteVideos() {
  if (!Object.keys(videos.remote).length) return;
  videos = { ...videos, remote: {} };
  emit();
}

export function setLocalVideo(track: MediaStreamTrack | null) {
  if (videos.local === track) return;
  videos = { ...videos, local: track };
  emit();
}

export function setLocalScreen(track: MediaStreamTrack | null) {
  if (videos.localScreen === track) return;
  videos = { ...videos, localScreen: track };
  emit();
}

/** A feed's track while it's coming in: yours with `self`. */
export function useVideoTrack(feed: string | undefined, self = false): MediaStreamTrack | null {
  return useSyncExternalStore(subscribe, () => {
    if (self) return feed && isScreen(feed) ? videos.localScreen : videos.local;
    return feed ? (videos.remote[feed]?.track ?? null) : null;
  });
}

// ───────────────────────── Sizes people want ─────────────────────────

const ORDER: Layer[] = ["off", "l", "m", "h"];
const views = new Map<number, { userId: string; layer: Layer }>();
let nextView = 0;
const wantListeners = new Set<() => void>();

/** The biggest size anything showing a feed wants. */
export function wanted(userId: string): Layer {
  let best: Layer = "off";
  for (const v of views.values()) if (v.userId === userId && ORDER.indexOf(v.layer) > ORDER.indexOf(best)) best = v.layer;
  return best;
}

export function onWantsChange(listener: () => void) {
  wantListeners.add(listener);
  return () => void wantListeners.delete(listener);
}

const wantsChanged = () => {
  for (const l of wantListeners) l();
};

/** The size that fits a picture this tall on screen (in device pixels): cameras are 1080 tall at most, screens up to 1440. */
export function layerFor(heightPx: number): Layer {
  if (heightPx <= 0) return "off";
  if (heightPx <= 240) return "l";
  if (heightPx <= 480) return "m";
  return "h";
}

/**
 * Asks for the size of `userId`'s camera that fits the element, for as
 * long as the element is on screen, following it as it resizes.
 */
export function useLayerFor(userId: string | undefined, ref: RefObject<HTMLElement | null>, enabled = true) {
  useEffect(() => {
    const el = ref.current;
    if (!userId || !el || !enabled) return;
    const id = nextView++;
    const set = (layer: Layer) => {
      if (views.get(id)?.layer === layer) return;
      views.set(id, { userId, layer });
      wantsChanged();
    };
    const measure = () => {
      const dpr = el.ownerDocument.defaultView?.devicePixelRatio ?? 1;
      set(layerFor(el.getBoundingClientRect().height * dpr));
    };
    measure();
    const Observer = el.ownerDocument.defaultView?.ResizeObserver ?? ResizeObserver;
    const observer = new Observer(measure);
    observer.observe(el);
    return () => {
      observer.disconnect();
      views.delete(id);
      wantsChanged();
    };
  }, [userId, ref, enabled]);
}

// ───────────────────────── Your camera ─────────────────────────

/**
 * Opens the camera picked in Voice & video settings, as sharp and smooth as
 * it goes under `ceiling` (lib/camera-quality.ts): your own choice unless
 * a call says otherwise.
 */
export async function openCamera(ceiling: Ceiling = ceilingOf(ownCeiling())): Promise<MediaStreamTrack> {
  if (!navigator.mediaDevices?.getUserMedia) throw new DOMException("No camera API", "NotSupportedError");
  const device = getPrefs().videoDevice;
  const ask = (most: boolean) =>
    navigator.mediaDevices.getUserMedia({
      audio: false,
      video: {
        deviceId: device ? { exact: device } : undefined,
        width: { ideal: widthFor(ceiling.height) },
        height: most ? { ideal: ceiling.height, max: ceiling.height } : { ideal: ceiling.height },
        frameRate: most ? { ideal: ceiling.fps, max: ceiling.fps } : { ideal: ceiling.fps },
      },
    });
  // A browser that can't scale this camera down to the ceiling gives the
  // nearest it has instead, and the encodings scale it down on the way out.
  const stream = await ask(true).catch((err: unknown) => {
    if (err instanceof DOMException && err.name === "OverconstrainedError") return ask(false);
    throw err;
  });
  const track = stream.getVideoTracks()[0];
  if (!track) throw new DOMException("No camera", "NotFoundError");
  track.contentHint = "motion";
  return track;
}

/** The ceiling you picked in Voice & video settings. */
export const ownCeiling = (): Ceiling => ({ height: getPrefs().cameraHeight, fps: getPrefs().cameraFps });

/**
 * The encodings for a camera track as it came: its own size and frame rate
 * (a camera may give less than asked), never above `ceiling`.
 */
export function encodingsFor(track: MediaStreamTrack | null, ceiling: Ceiling): RTCRtpEncodingParameters[] {
  const got = track?.getSettings() ?? {};
  const height = Math.min(got.height || ceiling.height, ceiling.height);
  const width = got.width && got.height ? Math.round((got.width * height) / got.height) : widthFor(height);
  // A camera taller than the ceiling is scaled down to it in every size.
  const over = got.height && got.height > height ? got.height / height : 1;
  return cameraEncodings(width, height, Math.min(Math.round(got.frameRate || ceiling.fps), ceiling.fps)).map((e) => ({
    ...e,
    scaleResolutionDownBy: (e.scaleResolutionDownBy ?? 1) * over,
  }));
}

/** Why the camera didn't open, in words. */
export function cameraProblem(err: unknown): string {
  const name = err instanceof DOMException ? err.name : "";
  if (name === "NotAllowedError" || name === "SecurityError") return tr("workspace.calls.camera.blocked");
  if (name === "NotFoundError" || name === "OverconstrainedError") return tr("workspace.calls.camera.notFound");
  if (name === "NotReadableError") return tr("workspace.calls.camera.busy");
  if (name === "NotSupportedError" || (typeof navigator !== "undefined" && !navigator.mediaDevices)) return tr("workspace.calls.camera.noHttps");
  return tr("workspace.calls.camera.failed");
}

// ───────────────────────── Your screen ─────────────────────────

export type SharedScreen = {
  video: MediaStreamTrack;
  /** Its sound, when asked for and the browser gave it. */
  audio: MediaStreamTrack | null;
  /** Why there's no sound, when it was asked for and none came. */
  silent: string | null;
};

/** How to share: with its sound or not, what the picker opens on, how sharp and smooth. */
export type ScreenAsk = { sound: boolean; surface: ShareSurface; quality: ShareQuality };

/**
 * Asks the browser for a screen, window or tab to share, at the size and
 * frame rate asked for (`surface` is only where its picker opens), and with
 * `sound` what it plays too. The sound is left as it is (no echo
 * cancelling or noise suppression, which are for voices and spoil music),
 * and never includes this page's own sound, so nobody hears the call back.
 */
export async function openScreen({ sound, surface, quality }: ScreenAsk): Promise<SharedScreen> {
  if (!navigator.mediaDevices?.getDisplayMedia) throw new DOMException("No screen sharing", "NotSupportedError");
  const asking = sound && canShareSound();
  const box = shareBox(quality.height);
  const stream = await navigator.mediaDevices.getDisplayMedia({
    audio: asking
      ? ({ echoCancellation: false, noiseSuppression: false, autoGainControl: false, suppressLocalAudioPlayback: false, restrictOwnAudio: true } as MediaTrackConstraints)
      : false,
    video: {
      width: { ideal: box.width, max: box.width },
      height: { ideal: box.height, max: box.height },
      frameRate: { ideal: quality.fps, max: quality.fps },
      displaySurface: surface,
    } as MediaTrackConstraints,
    // Chrome offers the whole system's sound for a whole screen where it can.
    ...(asking ? { systemAudio: "include" } : {}),
    // Tabs, windows and screens all stay offered; `surface` only picks where the picker opens.
    selfBrowserSurface: "exclude",
    surfaceSwitching: "include",
  } as DisplayMediaStreamOptions);
  const video = stream.getVideoTracks()[0];
  if (!video) throw new DOMException("No screen", "NotFoundError");
  // Text stays sharp, motion gives way first; at 60 it's the other way round (games, video).
  video.contentHint = wantsMotion(quality) ? "motion" : "detail";
  const audio = stream.getAudioTracks()[0] ?? null;
  if (audio) audio.contentHint = "music";
  const shared = (video.getSettings() as MediaTrackSettings & { displaySurface?: string }).displaySurface;
  const silent = !sound || audio ? null : !canShareSound() ? noSoundHere() : silentBecause(shared);
  return { video, audio, silent };
}

/** Said when this browser can't share sound at all. */
export const noSoundHere = () => tr("workspace.calls.screen.noSoundHere");

/** Why a share came without sound, by what was shared. */
function silentBecause(surface: string | undefined): string {
  if (surface === "browser") return tr("workspace.calls.screen.tabUnticked");
  if (surface === "window") return tr("workspace.calls.screen.window");
  return tr("workspace.calls.screen.system");
}

/** Chrome, Edge and the like. */
const chromium = () => {
  if (typeof navigator === "undefined") return false;
  const ua = navigator.userAgent;
  return /Chrome|Chromium|Edg\//.test(ua) && !/Firefox|FxiOS/.test(ua);
};

/** Whether this browser can share sound with a screen at all: Chrome and Edge can; Firefox and Safari share pictures only. */
export const canShareSound = chromium;

/** Whether this browser's picker opens where it's asked to (Chrome and Edge); Firefox and Safari choose for themselves. */
export const canPickSurface = chromium;

/** Whether this browser can share a screen at all (phones can't). */
export const canShareScreen = () => typeof navigator !== "undefined" && !!navigator.mediaDevices?.getDisplayMedia && !/Android|iPhone|iPad/.test(navigator.userAgent);

/** Why sharing didn't start, in words; null when you just closed the picker. */
export function screenProblem(err: unknown): string | null {
  const name = err instanceof DOMException ? err.name : "";
  if (name === "NotAllowedError" || name === "AbortError") return null;
  if (name === "NotSupportedError") return tr("workspace.calls.screen.unsupported");
  return tr("workspace.calls.screen.failed");
}
