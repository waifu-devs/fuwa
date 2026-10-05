import type { CSSProperties } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { hueOf } from "@/lib/format";
import { reportError } from "@/lib/reports";

/*
 * Server banners: where to keep their focus, and the color that goes with
 * them. A banner is saved at 1500×600 and shown at many other shapes (a
 * phone header, a Browse card), so the server keeps a focal point; and a
 * server can have an accent color, picked from its banner here in the
 * browser, that tints its welcome, applying and onboarding screens.
 */

export type BannerServer = Pick<Server, "id" | "name" | "iconUrl" | "bannerUrl" | "bannerFocusX" | "bannerFocusY" | "accentColor">;

/** 0xRRGGBB as #rrggbb. */
export const hex = (color: number) => `#${(color & 0xffffff).toString(16).padStart(6, "0")}`;

/** #rgb or #rrggbb as 0xRRGGBB, or null. */
export function parseHex(text: string): number | null {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(text.trim());
  if (!m) return null;
  const h = m[1]!.length === 3 ? [...m[1]!].map((c) => c + c).join("") : m[1]!;
  return parseInt(h, 16);
}

/** Where the banner's focus is, as CSS object-position. */
export const bannerPosition = (s: Pick<BannerServer, "bannerFocusX" | "bannerFocusY">) => `${clampPercent(s.bannerFocusX)}% ${clampPercent(s.bannerFocusY)}%`;

const clampPercent = (v: number) => (Number.isFinite(v) ? Math.min(100, Math.max(0, v)) : 50);

/**
 * The server's colors as CSS variables: `--accent-server` (its accent, or
 * a color from its hue) and `--h`, for the gradient banners without a picture.
 */
export function accentVars(server: Pick<BannerServer, "id" | "accentColor">): CSSProperties {
  const h = hueOf(server.id);
  const accent = server.accentColor !== undefined ? hex(server.accentColor) : `hsl(${h} 70% 58%)`;
  return { "--h": h, "--accent-server": accent } as CSSProperties;
}

/**
 * A few colors from a picture to offer as its accent, most vivid first: the
 * picture is drawn tiny, its pixels grouped by hue, and each group's average
 * color kept if it's lively enough to tint with. Works on files not uploaded
 * yet and on pictures from the instance the app is served by; other sites'
 * pictures can't be read, so they offer none.
 */
export async function bannerColors(src: string | Blob, count = 5): Promise<number[]> {
  if (typeof src === "string") return colorsAt(src, count);
  // A file not uploaded yet is read through a temporary URL, let go once it's been drawn.
  const url = URL.createObjectURL(src);
  try {
    return await colorsAt(url, count);
  } finally {
    URL.revokeObjectURL(url);
  }
}

async function colorsAt(url: string, count: number): Promise<number[]> {
  try {
    const image = await new Promise<HTMLImageElement>((resolve, reject) => {
      const img = new Image();
      img.decoding = "async";
      img.onload = () => resolve(img);
      img.onerror = () => reject(new Error("picture"));
      img.src = url;
    });
    const w = 48;
    const h = Math.max(1, Math.round((w * image.naturalHeight) / Math.max(1, image.naturalWidth)));
    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) return [];
    ctx.drawImage(image, 0, 0, w, h);
    return pickColors(ctx.getImageData(0, 0, w, h).data, count);
  } catch (err) {
    // A picture from another origin can't be read back (SecurityError): there's simply nothing to offer.
    if (!(err instanceof DOMException && err.name === "SecurityError")) reportError("BannerColors", "settings/welcome");
    return [];
  }
}

/** Groups RGBA pixels into 12 hue buckets and returns the liveliest averages. */
export function pickColors(pixels: Uint8ClampedArray, count: number): number[] {
  const buckets = Array.from({ length: 12 }, () => ({ r: 0, g: 0, b: 0, n: 0, sat: 0 }));
  for (let i = 0; i + 3 < pixels.length; i += 4) {
    if (pixels[i + 3]! < 128) continue;
    const [r, g, b] = [pixels[i]!, pixels[i + 1]!, pixels[i + 2]!];
    const max = Math.max(r, g, b);
    const min = Math.min(r, g, b);
    const light = (max + min) / 510;
    const sat = max === min ? 0 : (max - min) / (255 - Math.abs(max + min - 255));
    // Near-greys and near-black or white say nothing about the picture's color.
    if (sat < 0.25 || light < 0.12 || light > 0.9) continue;
    let hue = 0;
    if (max === r) hue = ((g - b) / (max - min) + 6) % 6;
    else if (max === g) hue = (b - r) / (max - min) + 2;
    else hue = (r - g) / (max - min) + 4;
    const bucket = buckets[Math.floor(hue * 2) % 12]!;
    bucket.r += r;
    bucket.g += g;
    bucket.b += b;
    bucket.sat += sat;
    bucket.n++;
  }
  return buckets
    .filter((b) => b.n >= 3)
    .sort((a, b) => b.n * (b.sat / b.n) - a.n * (a.sat / a.n))
    .slice(0, count)
    .map((b) => (Math.round(b.r / b.n) << 16) | (Math.round(b.g / b.n) << 8) | Math.round(b.b / b.n));
}
