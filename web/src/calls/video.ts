import { useEffect, useSyncExternalStore, type RefObject } from "react";
import { getPrefs } from "@/lib/prefs";

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

/** The size that fits a picture this tall on screen (in device pixels): a camera is 720 tall at most, a screen 1080. */
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

/** Opens the camera picked in Voice & video settings, at up to 720p and 30 frames a second. */
export async function openCamera(): Promise<MediaStreamTrack> {
  if (!navigator.mediaDevices?.getUserMedia) throw new DOMException("No camera API", "NotSupportedError");
  const device = getPrefs().videoDevice;
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: false,
    video: {
      deviceId: device ? { exact: device } : undefined,
      width: { ideal: 1280 },
      height: { ideal: 720 },
      frameRate: { ideal: 30, max: 30 },
    },
  });
  const track = stream.getVideoTracks()[0];
  if (!track) throw new DOMException("No camera", "NotFoundError");
  track.contentHint = "motion";
  return track;
}

/** Why the camera didn't open, in words. */
export function cameraProblem(err: unknown): string {
  const name = err instanceof DOMException ? err.name : "";
  if (name === "NotAllowedError" || name === "SecurityError") return "Your browser blocks the camera for fuwa. Allow it in the site's settings.";
  if (name === "NotFoundError" || name === "OverconstrainedError") return "No camera found. Plug one in, or pick another in Voice & video.";
  if (name === "NotReadableError") return "Another app is using the camera.";
  if (name === "NotSupportedError" || (typeof navigator !== "undefined" && !navigator.mediaDevices)) return "This page can't use a camera (it needs https).";
  return "The camera didn't start.";
}

/**
 * The three sizes a camera goes out in, smallest first (as browsers want
 * them): a quarter, half and full, each with a ceiling on its bitrate.
 */
export const ENCODINGS: RTCRtpEncodingParameters[] = [
  { rid: "l", scaleResolutionDownBy: 4, maxBitrate: 150_000, maxFramerate: 15 },
  { rid: "m", scaleResolutionDownBy: 2, maxBitrate: 500_000, maxFramerate: 30 },
  { rid: "h", scaleResolutionDownBy: 1, maxBitrate: 1_500_000, maxFramerate: 30 },
];

// ───────────────────────── Your screen ─────────────────────────

/** Asks the browser for a screen, window or tab to share, at up to 1080p. */
export async function openScreen(): Promise<MediaStreamTrack> {
  if (!navigator.mediaDevices?.getDisplayMedia) throw new DOMException("No screen sharing", "NotSupportedError");
  const stream = await navigator.mediaDevices.getDisplayMedia({
    audio: false,
    video: { width: { ideal: 1920, max: 1920 }, height: { ideal: 1080, max: 1080 }, frameRate: { ideal: 30, max: 30 } },
  });
  const track = stream.getVideoTracks()[0];
  if (!track) throw new DOMException("No screen", "NotFoundError");
  // Text stays sharp; motion gives way first.
  track.contentHint = "detail";
  return track;
}

/** Whether this browser can share a screen at all (phones can't). */
export const canShareScreen = () => typeof navigator !== "undefined" && !!navigator.mediaDevices?.getDisplayMedia && !/Android|iPhone|iPad/.test(navigator.userAgent);

/** Why sharing didn't start, in words; null when you just closed the picker. */
export function screenProblem(err: unknown): string | null {
  const name = err instanceof DOMException ? err.name : "";
  if (name === "NotAllowedError" || name === "AbortError") return null;
  if (name === "NotSupportedError") return "This browser can't share a screen.";
  return "Sharing your screen didn't start.";
}

/**
 * A shared screen's three sizes: a quarter for thumbnails, half, and full
 * for reading it, with more bits than a camera, since text needs them.
 */
export const SCREEN_ENCODINGS: RTCRtpEncodingParameters[] = [
  { rid: "l", scaleResolutionDownBy: 4, maxBitrate: 200_000, maxFramerate: 5 },
  { rid: "m", scaleResolutionDownBy: 2, maxBitrate: 700_000, maxFramerate: 15 },
  { rid: "h", scaleResolutionDownBy: 1, maxBitrate: 2_500_000, maxFramerate: 30 },
];
