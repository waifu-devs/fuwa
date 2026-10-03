import { ChannelType, type Channel } from "@/gen/fuwa/v1/types_pb";

/**
 * How a server's channels sit: the ones outside any category first, then each
 * category with its channels. The sidebar shows exactly this, and the server's
 * ReorderChannels takes it flattened (see `placements`).
 */
export type Layout = { loose: string[]; categories: { id: string; children: string[] }[] };

/** Where a dragged channel or category lands: before `before`, or at the end when it's null. */
export type Drop =
  | { kind: "channel"; id: string; parent: string; before: string | null }
  | { kind: "category"; id: string; before: string | null };

const isCategory = (c: Channel) => c.type === ChannelType.CATEGORY;

export function layoutOf(channels: Channel[]): Layout {
  const categories = channels.filter(isCategory);
  const known = new Set(categories.map((c) => c.id));
  return {
    loose: channels.filter((c) => !isCategory(c) && !known.has(c.parentId)).map((c) => c.id),
    categories: categories.map((category) => ({
      id: category.id,
      children: channels.filter((c) => !isCategory(c) && c.parentId === category.id).map((c) => c.id),
    })),
  };
}

/** The layout as the server takes it: every channel once, in display order, each with its category. */
export function placements(layout: Layout) {
  return [
    ...layout.loose.map((channelId) => ({ channelId, parentId: "" })),
    ...layout.categories.flatMap((c) => [
      { channelId: c.id, parentId: "" },
      ...c.children.map((channelId) => ({ channelId, parentId: c.id })),
    ]),
  ];
}

export const sameLayout = (a: Layout, b: Layout) => JSON.stringify(placements(a)) === JSON.stringify(placements(b));

const insert = (list: string[], id: string, before: string | null) => {
  const rest = list.filter((x) => x !== id);
  const at = before === null ? -1 : rest.indexOf(before);
  return at === -1 ? [...rest, id] : [...rest.slice(0, at), id, ...rest.slice(at)];
};

/** The layout after a drop. Dropping something where it already is gives back an equal layout. */
export function move(layout: Layout, drop: Drop): Layout {
  if (drop.kind === "category") {
    const moving = layout.categories.find((c) => c.id === drop.id);
    if (!moving) return layout;
    const ids = insert(
      layout.categories.map((c) => c.id),
      drop.id,
      drop.before,
    );
    return { ...layout, categories: ids.map((id) => layout.categories.find((c) => c.id === id)!) };
  }
  const without = (list: string[]) => list.filter((x) => x !== drop.id);
  const loose = without(layout.loose);
  const categories = layout.categories.map((c) => ({ ...c, children: without(c.children) }));
  if (!drop.parent) return { loose: insert(loose, drop.id, drop.before), categories };
  if (!categories.some((c) => c.id === drop.parent)) return layout;
  return {
    loose,
    categories: categories.map((c) => (c.id === drop.parent ? { ...c, children: insert(c.children, drop.id, drop.before) } : c)),
  };
}

/**
 * The channels as they'll be once the server takes a new order, for showing
 * it straight away. They keep the positions they had between them, handed
 * out in the new order, so the server's answer (and its events) usually
 * change nothing.
 */
export function arranged(channels: Channel[], order: { channelId: string; parentId: string }[]): Channel[] {
  const byId = new Map(channels.map((c) => [c.id, c]));
  const positions = channels.map((c) => c.position).sort((a, b) => a - b);
  const placed = order.flatMap(({ channelId, parentId }, n) => {
    const c = byId.get(channelId);
    if (!c) return [];
    return c.position === positions[n] && c.parentId === parentId ? [c] : [{ ...c, position: positions[n] ?? n, parentId }];
  });
  return placed.length === channels.length ? placed : channels;
}

/**
 * One step up or down, for the arrow keys: a channel moves past its neighbour,
 * and from the end of a category into the next one (or out of any, at the
 * top); a category moves past the next category. Null when it can't go further.
 */
export function step(layout: Layout, id: string, by: -1 | 1): Layout | null {
  const categoryAt = layout.categories.findIndex((c) => c.id === id);
  if (categoryAt !== -1) {
    const to = categoryAt + by;
    if (to < 0 || to >= layout.categories.length) return null;
    const ids = layout.categories.map((c) => c.id);
    return move(layout, { kind: "category", id, before: by < 0 ? ids[to]! : (ids[to + 1] ?? null) });
  }
  const groups = [{ id: "", children: layout.loose }, ...layout.categories];
  const g = groups.findIndex((x) => x.children.includes(id));
  if (g === -1) return null;
  const list = groups[g]!.children;
  const at = list.indexOf(id);
  if (at + by >= 0 && at + by < list.length) {
    return move(layout, { kind: "channel", id, parent: groups[g]!.id, before: by < 0 ? list[at - 1]! : (list[at + 2] ?? null) });
  }
  const next = groups[g + by];
  if (!next) return null;
  return move(layout, { kind: "channel", id, parent: next.id, before: by < 0 ? null : (next.children[0] ?? null) });
}
