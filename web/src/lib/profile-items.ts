import type { ProfileItem } from "@/gen/fuwa/v1/types_pb";
import { builtinEffect, sanitizeEffect, type EffectPicture, type ProfileEffectSpec } from "./effects/profile.ts";

/*
 * Profile items (docs/profile-items.md): the effects, decorations and nameplates an
 * instance and its servers offer, beyond the effects the app ships with.
 * Profiles and members name what they wear by id; this turns an id into
 * something to draw, from the lists the app keeps. Pure, so it's tested
 * without a browser.
 */

/** `ProfileItemKind`'s numbers, so this file needs nothing generated at runtime. */
export const ITEM_EFFECT = 1;
export const ITEM_DECORATION = 2;
export const ITEM_NAMEPLATE = 3;

/** Just what's read of an item, so plain objects work in tests. */
export type Item = Pick<ProfileItem, "id" | "kind" | "name" | "description" | "effect" | "pictureUrl" | "animated"> & { pictures?: readonly EffectPicture[] };

/** Parsed specs, by the item's id and its JSON, so one spec keeps one identity (effects restart when it changes). */
const specs = new Map<string, ProfileEffectSpec | null>();
const SPECS_KEPT = 256;

/**
 * An effect item's spec, ready to play: parsed and put through
 * `sanitizeEffect` like any spec that didn't ship with the app, under the
 * item's id in lowercase and its own name and description. Null when it
 * isn't an effect or nothing in it can play.
 */
export function itemEffect(item: Item): ProfileEffectSpec | null {
  if (item.kind !== ITEM_EFFECT || !item.effect) return null;
  const pictures = item.pictures ?? [];
  const key = `${item.id}\n${item.name}\n${item.description}\n${pictures.map((p) => p.url).join(" ")}\n${item.effect}`;
  const known = specs.get(key);
  if (known !== undefined) return known;
  const spec = parseEffect(item.effect, item.id.toLowerCase(), item, pictures);
  if (specs.size >= SPECS_KEPT) specs.delete(specs.keys().next().value!);
  specs.set(key, spec);
  return spec;
}

/** A spec from JSON text, with the pictures its image layers draw, or null when it isn't JSON or has nothing to play. `id` and `text` replace its own when given. */
export function parseEffect(json: string, id?: string, text?: { name: string; description: string }, pictures: readonly EffectPicture[] = []): ProfileEffectSpec | null {
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return null;
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const named = { ...(raw as Record<string, unknown>) };
  if (id !== undefined) named.id = id;
  if (text) {
    named.name = text.name;
    named.description = text.description;
  }
  return sanitizeEffect(named, pictures);
}

/** Why an effect someone pasted or picked can't be added: not JSON, or nothing in it the format knows. */
export type EffectProblem = "empty" | "json" | "spec";

/**
 * Checks an effect someone is about to add, before it goes to the instance
 * (which checks again, more strictly). The spec gets a stand-in id for the
 * preview; the instance gives it the item's. `pictures` are the ones it will
 * have (local files, as object URLs, until they're uploaded).
 */
export function checkEffectText(text: string, name = "", pictures: readonly EffectPicture[] = []): { spec: ProfileEffectSpec } | { problem: EffectProblem } {
  if (!text.trim()) return { problem: "empty" };
  try {
    JSON.parse(text);
  } catch {
    return { problem: "json" };
  }
  const spec = parseEffect(text, "preview", { name: name.trim() || "preview", description: "" }, pictures);
  return spec ? { spec } : { problem: "spec" };
}

/**
 * A spec's text split for editing: the JSON without its shader's code, and
 * the code on its own, so the WGSL is edited as WGSL rather than inside a
 * JSON string. Text that isn't JSON (or has no shader) comes back as it is.
 */
export function splitShader(text: string): { json: string; wgsl: string } {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return { json: text, wgsl: "" };
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return { json: text, wgsl: "" };
  const spec = raw as Record<string, unknown>;
  const shader = spec.shader;
  if (!shader || typeof shader !== "object" || typeof (shader as Record<string, unknown>).code !== "string") return { json: text, wgsl: "" };
  const { code, ...rest } = shader as Record<string, unknown>;
  const json = Object.keys(rest).length ? { ...spec, shader: rest } : Object.fromEntries(Object.entries(spec).filter(([k]) => k !== "shader"));
  return { json: specText(json), wgsl: code as string };
}

/**
 * The spec's text again from its two parts: the shader's code put back in
 * (its other settings, if any, kept), or the shader left out when there's
 * no code. JSON that doesn't parse comes back as it is, for the checks to
 * say so. A shader alone needs no JSON at all.
 */
export function joinShader(json: string, wgsl: string): string {
  const code = wgsl.replace(/\s+$/, "");
  if (!json.trim()) return code.trim() ? JSON.stringify({ layers: [], shader: { code } }) : "";
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return json;
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return json;
  const spec = { ...(raw as Record<string, unknown>) };
  const settings = spec.shader && typeof spec.shader === "object" ? (spec.shader as Record<string, unknown>) : {};
  if (code.trim()) spec.shader = { ...settings, code };
  else delete spec.shader;
  if (!Array.isArray(spec.layers)) spec.layers = [];
  return JSON.stringify(spec);
}

/** The items of one kind, as offered. */
export const itemsOfKind = <T extends Item>(items: readonly T[] | undefined, kind: number): T[] => (items ?? []).filter((i) => i.kind === kind);

/** An item by id, of a kind (ids are ULIDs; matched whatever their case, since an effect's spec keeps it in lowercase). */
function find<T extends Item>(items: readonly T[] | undefined, id: string, kind: number): T | undefined {
  if (!id || !items) return undefined;
  const upper = id.toUpperCase();
  return items.find((i) => i.kind === kind && (i.id === id || i.id.toUpperCase() === upper));
}

/** An effect by id: one the app ships with, else one of the items given. Nothing for an id it can't find. */
export function resolveEffect(id: string | undefined, items: readonly Item[] | undefined): ProfileEffectSpec | undefined {
  if (!id) return undefined;
  const builtin = builtinEffect(id);
  if (builtin) return builtin;
  const item = find(items, id, ITEM_EFFECT);
  return (item && itemEffect(item)) ?? undefined;
}

/** A decoration by id among the items given. */
export const resolveDecoration = <T extends Item>(id: string | undefined, items: readonly T[] | undefined): T | undefined =>
  id ? find(items, id, ITEM_DECORATION) : undefined;

/** A nameplate by id among the items given. */
export const resolveNameplate = <T extends Item>(id: string | undefined, items: readonly T[] | undefined): T | undefined =>
  id ? find(items, id, ITEM_NAMEPLATE) : undefined;

/** Where each list comes from: the instance's, and the server's the person is seen in (if any). */
export type ItemLists<T extends Item = Item> = { instance: readonly T[] | undefined; server: readonly T[] | undefined };

/**
 * The effect someone shows: their server profile's pick where they're seen
 * in a server (a built-in or one of that server's), else their own (a
 * built-in or one of the instance's).
 */
export function wornEffect(own: string | undefined, member: { effect: string } | undefined, lists: ItemLists): ProfileEffectSpec | undefined {
  if (member?.effect) return resolveEffect(member.effect, lists.server);
  return resolveEffect(own, lists.instance);
}

/** The decoration someone shows: their server profile's (one of that server's) where they have one, else their own (one of the instance's). */
export function wornDecoration<T extends Item>(
  user: { decorationId: string } | undefined,
  member: { decorationId: string } | undefined,
  lists: ItemLists<T>,
): T | undefined {
  if (member?.decorationId) return resolveDecoration(member.decorationId, lists.server);
  return resolveDecoration(user?.decorationId, lists.instance);
}

/** The nameplate someone shows, as `wornDecoration`: their server profile's where they have one, else their own. */
export function wornNameplate<T extends Item>(
  user: { nameplateId: string } | undefined,
  member: { nameplateId: string } | undefined,
  lists: ItemLists<T>,
): T | undefined {
  if (member?.nameplateId) return resolveNameplate(member.nameplateId, lists.server);
  return resolveNameplate(user?.nameplateId, lists.instance);
}

/** Items drawn from one picture of their own: decorations and nameplates. */
export const isPictureItem = (item: Pick<Item, "kind">) => item.kind === ITEM_DECORATION || item.kind === ITEM_NAMEPLATE;

/** A name for an item from a file's name: no extension, separators as spaces, within the 40 characters an item's name may have. */
export function nameFromFile(file: string): string {
  const base = file.replace(/\.[^.]+$/, "").replace(/[_-]+/g, " ").replace(/\s+/g, " ").trim();
  return base.slice(0, 40).trim();
}

// ───────────────────────── Helping people make them ─────────────────────────

/** The docs that explain each kind: the effect format, and what a decoration should look like. */
export const EFFECT_DOCS = "https://github.com/waifu-devs/fuwa/blob/master/docs/profile-effects.md#the-format";
export const DECORATION_DOCS = "https://github.com/waifu-devs/fuwa/blob/master/docs/profile-items.md#decorations";
export const NAMEPLATE_DOCS = "https://github.com/waifu-devs/fuwa/blob/master/docs/profile-items.md#nameplates";

/**
 * A built-in effect as a spec to start from: only its layers (the item gets
 * its own id, name and line), laid out to read, with each list of numbers
 * or colors on one line. The desktop's `starter_text` writes the same.
 */
export function starterText(id: string): string | undefined {
  const spec = builtinEffect(id);
  return spec && specText({ layers: spec.layers });
}

/** JSON with two-space indents, short lists (ranges, colors) kept on one line. */
export function specText(value: unknown): string {
  return JSON.stringify(value, null, 2).replace(/\[\n\s*([^[\]{}]*?)\n\s*\]/g, (_, inner: string) => `[${inner.split(/,\n\s*/).join(", ")}]`);
}

/**
 * What someone pastes into an AI chat to have it write a profile effect: the
 * format, its limits and an example, ending where they describe what they
 * want. The desktop's `EFFECT_PROMPT` is this word for word.
 */
export const EFFECT_PROMPT = `You're writing a profile effect for fuwa, a chat app: an animated layer of small particles that plays over someone's profile card (304 by 420 pixels) when it opens. People still need to read the name, status and bio under it, so particles start at the card's edges and corners and cross it quickly.

An effect is a JSON spec, data rather than code. It plays a short intro (under three seconds) when the card opens, then settles into a sparse idle loop for as long as the card stays open.

{
  "layers": [ ... 1 to 6 layers ... ]
}

Each layer is one kind of particle. Its fields:
- shape: one of petal, star, sparkle, heart, snowflake, bubble, dot, confetti, streak.
- motion: fall or rise (across the card, swaying), twinkle (a glint in place, then a rest), drift (wanders and glows), burst (flies out and falls away), shoot (a streak, then a rest), pop (swells and pops).
- phase: intro plays once when the card opens; idle loops after it. Have at least one of each.
- count: 1 to 24 particles, for the 304 by 420 card. At most 60 play at once across all layers, so keep idle layers sparse (5 to 12).
- from: where they start: top, bottom, edges, corners, anywhere, top-left, top-right, center.
- size: [min, max] in pixels, 2 to 96. Most particles look right between 10 and 32.
- duration: [min, max] for one pass, 300 to 20000 ms. Intro layers 900 to 2500; idle layers 2000 to 12000.
- delay: [min, max] in ms, 0 to 3000: when each intro particle starts after the card opens. Leave it out for idle layers.
- colors: 1 to 8 of #rrggbb or the theme's tokens primary, ring, accent, foreground, card, or profile (the card's own color). Mixing a token in makes the effect fit every theme.
- spin: degrees turned over a pass, -1440 to 1440 (optional).
- sway: 0 to 400 pixels: sideways sway for fall and rise (10 to 30), wander radius for drift, flight distance for burst (200 to 300) (optional).
- flip: true to tumble in 3D, like paper (optional).
- glow: true for a soft glow in the particle's color (optional).
- opacity: peak opacity, 0.05 to 1 (optional, 1 when left out).

Two more things an effect can have, if what I want calls for them:

- Pictures: an effect can carry up to 8 pictures of its own, uploaded with it in an order. A layer with "image": 0 draws the first picture instead of a shape ("image": 1 the second, and so on). It leaves out shape, and may leave out colors: a picture keeps its own, and its glow is then the theme's primary. Only use pictures I say I have, and tell me which picture goes in which place.
- A shader: "shader": { "code": "...", "intensity": 0.6, "speed": 1 } draws a WGSL shader across the whole card, under the particles. The code defines fn shade(uv: vec2f) -> vec4f, called for each pixel with uv from 0,0 at the top left to 1,1 at the bottom right, and returns a color and how much it covers the card (straight alpha, 0 to 1). Keep alpha low in the middle (0.15 or less) so the text stays readable. It can read the uniform fuwa: fuwa.primary (the theme's color), fuwa.accent (the card's own color), fuwa.background (the card), fuwa.foreground (its text), all vec4f; fuwa.res (its size in pixels) and fuwa.pointer (0 to 1), both vec2f; fuwa.time (seconds) and fuwa.intensity, both f32; and the helpers hash21(p: vec2f) -> f32, noise(p: vec2f) -> f32 and fbm(p: vec2f) -> f32. It can't declare anything with an attribute (no @group, @binding or entry points), turn on extensions or contain links, and it's at most 16 KB. intensity (0.05 to 1) scales how much it covers, speed (0 to 4) how fast its time runs. A shader alone, with "layers": [], is an effect too. In the JSON the code is one string, so write its line breaks as \\n.

fuwa refuses a spec with a shape, motion, place or color not on these lists, a number out of its range, a shader that breaks those rules, or more than 40 KB, and drops fields it doesn't know. The effect gets its id, name and description from the item it's added as, so leave those out.

An example, fuwa's own Sakura (a gust of petals from the top left, then a gentle fall):

{
  "layers": [
    { "shape": "petal", "motion": "burst", "phase": "intro", "count": 18, "from": "top-left", "size": [18, 28], "duration": [1500, 2300], "delay": [0, 260], "colors": ["#ffb7c5", "#ff8fab", "#ffc8d6", "primary"], "spin": 540, "sway": 280, "flip": true, "opacity": 0.95 },
    { "shape": "petal", "motion": "fall", "phase": "idle", "count": 9, "from": "top", "size": [15, 24], "duration": [6500, 9500], "colors": ["#ffb7c5", "#ff8fab", "#ffc8d6", "primary"], "spin": 320, "sway": 22, "flip": true, "opacity": 0.9 }
  ]
}

Answer with the whole spec in one json code block, then say in a sentence or two which numbers to change to tune it. If I come back with an error from fuwa, fix it and answer with the whole spec again.

What I want:
`;

/**
 * What someone pastes into an AI that makes pictures to have it draw an
 * avatar decoration, ending where they describe it. The desktop's
 * `DECORATION_PROMPT` is this word for word.
 */
export const DECORATION_PROMPT = `Draw an avatar decoration for fuwa, a chat app: a frame that's drawn over someone's round profile picture, like a wreath or a crown around it.

- A square PNG, 600 by 600 pixels, with a truly transparent background (real alpha, not a checkerboard pattern painted in, and no background color).
- The profile picture is a circle 500 pixels across in the middle, 50 pixels in from each edge. Keep that circle see-through so the face shows: put the art around its rim and in the corners, overlapping the circle's edge only a little.
- Don't draw a face, a person or a profile picture inside it: only the frame.
- It's shown as small as 40 pixels across next to messages, so use bold shapes and clear outlines, and no text or fine detail.

What I want:
`;

/**
 * What someone pastes into an AI that makes pictures to have it draw a
 * nameplate, ending where they describe it. The desktop's
 * `NAMEPLATE_PROMPT` is this word for word.
 */
export const NAMEPLATE_PROMPT = `Draw a nameplate for fuwa, a chat app: a wide decorative strip shown behind someone's name in the member list, like a little banner.

- A PNG, 900 by 150 pixels (six times as wide as it is tall). It may be see-through or fill the whole strip.
- Someone's avatar and name sit over its left half, and the app fades that half out, so put the art mostly on the right and keep the left calm: soft color, no busy detail.
- It's shown about 240 by 44 pixels, so use bold shapes and clear colors, and no text, faces or fine detail.

What I want:
`;

/** A nameplate's size, as the prompt and the docs say. */
export const NAMEPLATE_WIDTH = 900;
export const NAMEPLATE_HEIGHT = 150;

/** The template's size and the avatar circle in it, as the decoration prompt and the docs say: the avatar is 1/1.2 of the picture. */
export const TEMPLATE_SIZE = 600;
export const TEMPLATE_AVATAR = 500;
