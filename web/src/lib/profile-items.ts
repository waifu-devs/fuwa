import type { ProfileItem } from "@/gen/fuwa/v1/types_pb";
import { builtinEffect, sanitizeEffect, type ProfileEffectSpec } from "./effects/profile.ts";

/*
 * Profile items (docs/profile-items.md): the effects and decorations an
 * instance and its servers offer, beyond the effects the app ships with.
 * Profiles and members name what they wear by id; this turns an id into
 * something to draw, from the lists the app keeps. Pure, so it's tested
 * without a browser.
 */

/** `ProfileItemKind`'s numbers, so this file needs nothing generated at runtime. */
export const ITEM_EFFECT = 1;
export const ITEM_DECORATION = 2;

/** Just what's read of an item, so plain objects work in tests. */
export type Item = Pick<ProfileItem, "id" | "kind" | "name" | "description" | "effect" | "pictureUrl" | "animated">;

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
  const key = `${item.id}\n${item.name}\n${item.description}\n${item.effect}`;
  const known = specs.get(key);
  if (known !== undefined) return known;
  const spec = parseEffect(item.effect, item.id.toLowerCase(), item);
  if (specs.size >= SPECS_KEPT) specs.delete(specs.keys().next().value!);
  specs.set(key, spec);
  return spec;
}

/** A spec from JSON text, or null when it isn't JSON or has nothing to play. `id` and `text` replace its own when given. */
export function parseEffect(json: string, id?: string, text?: { name: string; description: string }): ProfileEffectSpec | null {
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
  return sanitizeEffect(named);
}

/** Why an effect someone pasted or picked can't be added: not JSON, or nothing in it the format knows. */
export type EffectProblem = "empty" | "json" | "spec";

/**
 * Checks an effect someone is about to add, before it goes to the instance
 * (which checks again, more strictly). The spec gets a stand-in id for the
 * preview; the instance gives it the item's.
 */
export function checkEffectText(text: string, name = ""): { spec: ProfileEffectSpec } | { problem: EffectProblem } {
  if (!text.trim()) return { problem: "empty" };
  try {
    JSON.parse(text);
  } catch {
    return { problem: "json" };
  }
  const spec = parseEffect(text, "preview", { name: name.trim() || "preview", description: "" });
  return spec ? { spec } : { problem: "spec" };
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

/** A name for an item from a file's name: no extension, separators as spaces, within the 40 characters an item's name may have. */
export function nameFromFile(file: string): string {
  const base = file.replace(/\.[^.]+$/, "").replace(/[_-]+/g, " ").replace(/\s+/g, " ").trim();
  return base.slice(0, 40).trim();
}
