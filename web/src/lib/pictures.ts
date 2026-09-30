/**
 * Getting a picture ready to upload: what each kind is cropped to, the crop
 * itself (a center point and a zoom over the image), and drawing the result.
 */

export type PictureKind = "avatar" | "banner" | "icon";

/** The size each kind is saved at, and the shape people see it in. */
export const PICTURE: Record<PictureKind, { width: number; height: number; round: boolean; label: string }> = {
  avatar: { width: 512, height: 512, round: true, label: "avatar" },
  banner: { width: 1500, height: 600, round: false, label: "banner" },
  icon: { width: 512, height: 512, round: false, label: "server icon" },
};

/** What pictures can be, as the server takes them. */
export const PICTURE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/avif"];

/** The largest zoom the cropper allows. */
export const MAX_ZOOM = 4;

/** A crop: the image point at the frame's center, in image pixels, and a zoom of 1 or more. */
export type Crop = { cx: number; cy: number; zoom: number };

type Size = { width: number; height: number };

/** The scale that makes the image just cover the frame, at zoom 1. */
export const coverScale = (image: Size, frame: Size) => Math.max(frame.width / image.width, frame.height / image.height);

/** Keeps the frame inside the image: zoom within bounds, center far enough from the edges. */
export function clampCrop(crop: Crop, image: Size, frame: Size): Crop {
  const zoom = Math.min(MAX_ZOOM, Math.max(1, crop.zoom));
  const scale = coverScale(image, frame) * zoom;
  const halfW = frame.width / 2 / scale;
  const halfH = frame.height / 2 / scale;
  const within = (v: number, lo: number, hi: number) => (lo > hi ? (lo + hi) / 2 : Math.min(hi, Math.max(lo, v)));
  return { zoom, cx: within(crop.cx, halfW, image.width - halfW), cy: within(crop.cy, halfH, image.height - halfH) };
}

/** Where to draw the image in the frame for a crop: its top-left corner and scale, in frame pixels. */
export function placement(crop: Crop, image: Size, frame: Size) {
  const scale = coverScale(image, frame) * crop.zoom;
  return { x: frame.width / 2 - crop.cx * scale, y: frame.height / 2 - crop.cy * scale, scale };
}

/** Zooms by `factor` keeping the image point under `at` (in frame pixels) where it is. */
export function zoomAround(crop: Crop, factor: number, at: { x: number; y: number }, image: Size, frame: Size): Crop {
  const before = placement(crop, image, frame);
  const px = (at.x - before.x) / before.scale;
  const py = (at.y - before.y) / before.scale;
  const zoom = Math.min(MAX_ZOOM, Math.max(1, crop.zoom * factor));
  const scale = coverScale(image, frame) * zoom;
  return clampCrop({ zoom, cx: px - (at.x - frame.width / 2) / scale, cy: py - (at.y - frame.height / 2) / scale }, image, frame);
}

export const centered = (image: Size): Crop => ({ cx: image.width / 2, cy: image.height / 2, zoom: 1 });

export function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("that file isn't a picture this browser can open"));
    image.src = src;
  });
}

/**
 * Draws the cropped part of the image at the kind's size, as WebP where the
 * browser can make it (much smaller) and PNG where it can't.
 */
export async function cropped(image: HTMLImageElement, crop: Crop, kind: PictureKind, frame: Size): Promise<Blob> {
  const out = PICTURE[kind];
  const scale = coverScale(image, frame) * crop.zoom;
  const sw = frame.width / scale;
  const sh = frame.height / scale;
  const canvas = document.createElement("canvas");
  canvas.width = out.width;
  canvas.height = out.height;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("this browser can't crop pictures");
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(image, crop.cx - sw / 2, crop.cy - sh / 2, sw, sh, 0, 0, out.width, out.height);
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/webp", 0.9));
  if (blob && blob.type === "image/webp") return blob;
  const png = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  if (!png) throw new Error("this browser couldn't save the crop");
  return png;
}
