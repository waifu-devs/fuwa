import { savePicture } from "@/components/chat/ImageViewer";
import { IMAGE_FONT, shareImage, type ImageLabels, type ShareCards } from "@/lib/gacha-share";

/** How much sharper than its layout the saved picture is. */
const SCALE = 2;

/**
 * Saves a picture of a pull's cards as a PNG: the shapes from `shareImage`,
 * then its words written in the app's own font (a picture drawn from an SVG
 * can't use the page's fonts).
 */
export async function savePullImage(pulled: ShareCards, labels: ImageLabels, name: string): Promise<void> {
  const image = shareImage(pulled, labels);
  await Promise.all([400, 700, 800].map((w) => document.fonts.load(`${w} 16px "${IMAGE_FONT}"`).catch(() => [])));
  const shapes = new Image();
  const svgUrl = URL.createObjectURL(new Blob([image.shapes], { type: "image/svg+xml" }));
  try {
    shapes.src = svgUrl;
    await shapes.decode();
    const canvas = document.createElement("canvas");
    canvas.width = image.width * SCALE;
    canvas.height = image.height * SCALE;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("no canvas");
    ctx.scale(SCALE, SCALE);
    ctx.drawImage(shapes, 0, 0, image.width, image.height);
    ctx.textBaseline = "alphabetic";
    for (const t of image.texts) {
      ctx.font = `${t.weight} ${t.size}px "${IMAGE_FONT}", ui-rounded, sans-serif`;
      ctx.fillStyle = t.color;
      ctx.globalAlpha = t.opacity;
      ctx.textAlign = t.anchor === "middle" ? "center" : t.anchor === "end" ? "right" : "left";
      ctx.fillText(t.text, t.x, t.y);
    }
    const blob = await new Promise<Blob | null>((done) => canvas.toBlob(done, "image/png"));
    if (!blob) throw new Error("no picture");
    const href = URL.createObjectURL(blob);
    savePicture(href, name);
    setTimeout(() => URL.revokeObjectURL(href), 60_000);
  } finally {
    URL.revokeObjectURL(svgUrl);
  }
}

/** A file name for a pull's picture: the banner's name, kept to safe characters. */
export function imageName(bannerName: string): string {
  const base = bannerName
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .replace(/[^\p{L}\p{N}]+/gu, "-")
    .replace(/^-+|-+$/g, "")
    .toLowerCase();
  return `${base || "gacha"}-pull.png`;
}
