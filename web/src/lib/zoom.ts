/*
 * The picture viewer's zoom and pan (components/chat/ImageViewer.tsx), kept
 * pure so it's tested on its own. A picture is first laid out fitted to the
 * room it has (never stretched past its own size); `scale` is how much
 * bigger than that it's drawn, 1 being fitted, and `x`/`y` how far its
 * middle is moved from the room's middle, in screen pixels. The desktop app
 * has the same rules (desktop/src/core/zoom.rs).
 */

export type View = { scale: number; x: number; y: number };
export type Size = { width: number; height: number };

/** As it opens: fitted and in the middle. */
export const FITTED: View = { scale: 1, x: 0, y: 0 };

/** How far in a picture can go past its own size, and never less than this past fitted. */
const MOST_PAST_ACTUAL = 4;
const MOST_PAST_FITTED = 8;

/** The size a picture is laid out at in `room`: its own, or smaller to fit. */
export function fitted(natural: Size, room: Size): Size {
  if (!(natural.width > 0 && natural.height > 0)) return { width: room.width, height: room.height };
  const scale = Math.min(1, room.width / natural.width, room.height / natural.height);
  return { width: natural.width * scale, height: natural.height * scale };
}

/** The scale that draws the picture one screen pixel per pixel of its own (1:1). */
export function actualScale(natural: Size, room: Size): number {
  const fit = fitted(natural, room);
  return fit.width > 0 ? Math.max(1, natural.width / fit.width) : 1;
}

/** The closest it can zoom in. */
export function mostScale(natural: Size, room: Size): number {
  return Math.max(MOST_PAST_FITTED, actualScale(natural, room) * MOST_PAST_ACTUAL);
}

/**
 * Keeps the picture where it can be seen: no smaller than fitted, no bigger
 * than `mostScale`, and moved no further than takes its edge to the room's
 * (centered on an axis where it's smaller than the room).
 */
export function clampView(view: View, natural: Size, room: Size): View {
  const scale = Math.min(mostScale(natural, room), Math.max(1, view.scale));
  const fit = fitted(natural, room);
  const roomX = Math.max(0, (fit.width * scale - room.width) / 2);
  const roomY = Math.max(0, (fit.height * scale - room.height) / 2);
  // `|| 0` so a centered picture is at 0, never -0.
  return { scale, x: Math.min(roomX, Math.max(-roomX, view.x)) || 0, y: Math.min(roomY, Math.max(-roomY, view.y)) || 0 };
}

/**
 * Zooms to `scale` keeping the point under `at` (from the room's middle,
 * in screen pixels) where it is, as a wheel or a pinch does.
 */
export function zoomAt(view: View, scale: number, at: { x: number; y: number }, natural: Size, room: Size): View {
  const next = Math.min(mostScale(natural, room), Math.max(1, scale));
  const k = next / view.scale;
  return clampView({ scale: next, x: at.x - (at.x - view.x) * k, y: at.y - (at.y - view.y) * k }, natural, room);
}

/** Moves a zoomed picture by a drag of `dx`, `dy`. */
export function panBy(view: View, dx: number, dy: number, natural: Size, room: Size): View {
  return clampView({ ...view, x: view.x + dx, y: view.y + dy }, natural, room);
}

/**
 * A double click: back to fitted when zoomed, otherwise to 1:1 (or twice
 * fitted, for a picture already shown at its own size) around the point.
 */
export function toggleAt(view: View, at: { x: number; y: number }, natural: Size, room: Size): View {
  if (view.scale > 1.01) return FITTED;
  return zoomAt(view, Math.max(2, actualScale(natural, room)), at, natural, room);
}

/** How much one notch of a wheel (or a trackpad pinch, which browsers send as a wheel with Ctrl) zooms. */
export function wheelFactor(deltaY: number, pinch: boolean): number {
  const step = Math.max(-100, Math.min(100, deltaY));
  return Math.exp(-step * (pinch ? 0.01 : 0.002));
}

/** The +, - buttons and keys: a fixed step around the middle. */
export const STEP = 1.5;

/** The one after or before `index` among `count`, going round. */
export function stepIndex(index: number, count: number, by: 1 | -1): number {
  return count > 0 ? (index + by + count) % count : 0;
}
