import { useEffect, useSyncExternalStore, type RefObject } from "react";
import { getPrefs } from "@/lib/prefs";

/**
 * Cameras in your call: everyone else's as their tracks arrive, and yours.
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

type Videos = { remote: Record<string, RemoteVideo>; local: MediaStreamTrack | null };

let videos: Videos = { remote: {}, local: null };
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

/** Someone's camera track while it's coming in: yours with `self`. */
export function useVideoTrack(userId: string | undefined, self = false): MediaStreamTrack | null {
  return useSyncExternalStore(subscribe, () => (self ? videos.local : userId ? (videos.remote[userId]?.track ?? null) : null));
}

// ───────────────────────── Sizes people want ─────────────────────────

const ORDER: Layer[] = ["off", "l", "m", "h"];
const views = new Map<number, { userId: string; layer: Layer }>();
let nextView = 0;
const wantListeners = new Set<() => void>();

/** The biggest size anything showing `userId`'s camera wants. */
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

/** The size that fits a picture this tall on screen (in device pixels): a camera is 720 tall at most. */
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
