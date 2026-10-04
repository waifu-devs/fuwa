/**
 * How your servers sit on the rail, on one instance: top to bottom, each a
 * server or a folder of them. The instance keeps it on your account
 * (`AccountService.GetServerArrangement`), so every device shows the same.
 *
 * Kept free of the generated types so tests run on plain Node.
 */

export type RailFolder = { id: string; name: string; color: number; servers: string[] };
export type RailEntry = { kind: "server"; id: string } | { kind: "folder"; folder: RailFolder };
export type RailLayout = RailEntry[];

/**
 * Where a dragged server or folder lands.
 *
 * - `server`: into `folder` ("" for none), before `before` (a server in that
 *   folder, or a top-level server or folder id when `folder` is ""), at the
 *   end when it's null
 * - `combine`: onto the top-level server `with`, making a folder of the two
 * - `folder`: a folder before the top-level entry `before`, or at the end
 */
export type RailDrop =
  | { kind: "server"; id: string; folder: string; before: string | null }
  | { kind: "combine"; id: string; with: string }
  | { kind: "folder"; id: string; before: string | null };

/** Folder names, as the instance takes them. */
export const FOLDER_NAME_MAX = 32;

/** Folder colors to pick from, 0xRRGGBB; 0 is the theme's accent. */
export const FOLDER_COLORS = [0, 0xff6b9d, 0xff8a5c, 0xf5c044, 0x6ad48a, 0x4cc9d6, 0x6c8dff, 0xa77bff, 0x8a8f9c] as const;

export const keyOf = (e: RailEntry) => (e.kind === "server" ? e.id : e.folder.id);

/** Every server in the layout, top to bottom. */
export const serversIn = (layout: RailLayout) => layout.flatMap((e) => (e.kind === "server" ? [e.id] : e.folder.servers));

/**
 * The layout to show: the saved one without servers you're no longer in,
 * then servers it doesn't place, in the order you joined them.
 */
export function railLayout(saved: RailLayout | null, servers: string[]): RailLayout {
  const here = new Set(servers);
  const seen = new Set<string>();
  const keep = (id: string) => here.has(id) && !seen.has(id) && (seen.add(id), true);
  const placed: RailLayout = [];
  for (const e of saved ?? []) {
    if (e.kind === "server") {
      if (keep(e.id)) placed.push(e);
    } else {
      const kept = e.folder.servers.filter(keep);
      if (kept.length) placed.push(kept.length === e.folder.servers.length ? e : { kind: "folder", folder: { ...e.folder, servers: kept } });
    }
  }
  for (const id of servers) if (keep(id)) placed.push({ kind: "server", id });
  return placed;
}

export const sameRail = (a: RailLayout, b: RailLayout) => JSON.stringify(a) === JSON.stringify(b);

const insert = <T>(list: T[], item: T, at: number) => (at < 0 || at > list.length ? [...list, item] : [...list.slice(0, at), item, ...list.slice(at)]);

/** The layout after a drop. Dropping something where it already is gives back an equal layout. */
export function moveRail(layout: RailLayout, drop: RailDrop, newId: () => string = folderId): RailLayout {
  if (drop.kind === "folder") {
    const at = layout.findIndex((e) => e.kind === "folder" && e.folder.id === drop.id);
    if (at === -1 || drop.before === drop.id) return layout;
    const moving = layout[at]!;
    const rest = layout.filter((_, n) => n !== at);
    return insert(rest, moving, drop.before === null ? -1 : rest.findIndex((e) => keyOf(e) === drop.before));
  }
  const id = drop.id;
  if (!serversIn(layout).includes(id)) return layout;
  if (drop.kind === "combine" && drop.with === id) return layout;
  if (drop.kind === "server" && drop.before === id) return layout;
  // Out of wherever it was; a folder left empty goes too.
  const rest: RailLayout = layout.flatMap((e): RailLayout => {
    if (e.kind === "server") return e.id === id ? [] : [e];
    if (!e.folder.servers.includes(id)) return [e];
    const servers = e.folder.servers.filter((s) => s !== id);
    return servers.length ? [{ kind: "folder", folder: { ...e.folder, servers } }] : [];
  });
  if (drop.kind === "combine") {
    const at = rest.findIndex((e) => e.kind === "server" && e.id === drop.with);
    if (at === -1) return layout;
    const folder: RailFolder = { id: newId(), name: "", color: 0, servers: [drop.with, id] };
    return rest.map((e, n) => (n === at ? { kind: "folder", folder } : e));
  }
  if (!drop.folder) {
    return insert(rest, { kind: "server", id }, drop.before === null ? -1 : rest.findIndex((e) => keyOf(e) === drop.before));
  }
  if (!rest.some((e) => e.kind === "folder" && e.folder.id === drop.folder)) return layout;
  return rest.map((e) => {
    if (e.kind !== "folder" || e.folder.id !== drop.folder) return e;
    const servers = insert(e.folder.servers, id, drop.before === null ? -1 : e.folder.servers.indexOf(drop.before));
    return { kind: "folder", folder: { ...e.folder, servers } };
  });
}

/**
 * One step up or down, for the keyboard. A server moves past its neighbour,
 * into an open folder it meets (and out of one at its ends); past a closed
 * folder it goes by. A folder moves past the next top-level entry. Null when
 * it can't go further.
 */
export function stepRail(layout: RailLayout, id: string, by: -1 | 1, open: (folder: string) => boolean): RailLayout | null {
  const top = layout.findIndex((e) => keyOf(e) === id);
  if (top !== -1 && layout[top]!.kind === "folder") {
    const to = top + by;
    if (to < 0 || to >= layout.length) return null;
    return moveRail(layout, { kind: "folder", id, before: by < 0 ? keyOf(layout[to]!) : (layout[to + 1] ? keyOf(layout[to + 1]!) : null) });
  }
  if (top !== -1) {
    const next = layout[top + by];
    if (!next) return null;
    if (next.kind === "folder" && open(next.folder.id)) {
      const servers = next.folder.servers;
      return moveRail(layout, { kind: "server", id, folder: next.folder.id, before: by > 0 ? (servers[0] ?? null) : null });
    }
    const after = layout[top + 2];
    return moveRail(layout, { kind: "server", id, folder: "", before: by < 0 ? keyOf(next) : after ? keyOf(after) : null });
  }
  const at = layout.findIndex((e) => e.kind === "folder" && e.folder.servers.includes(id));
  if (at === -1) return null;
  const folder = (layout[at] as Extract<RailEntry, { kind: "folder" }>).folder;
  const n = folder.servers.indexOf(id);
  if (n + by >= 0 && n + by < folder.servers.length) {
    return moveRail(layout, { kind: "server", id, folder: folder.id, before: by < 0 ? folder.servers[n - 1]! : (folder.servers[n + 2] ?? null) });
  }
  // Out of the folder: just above it, or just below.
  const below = layout[at + 1];
  return moveRail(layout, { kind: "server", id, folder: "", before: by < 0 ? folder.id : below ? keyOf(below) : null });
}

/** Changes one folder; `null` dissolves it, leaving its servers where it was. */
export function editFolder(layout: RailLayout, id: string, change: Partial<Omit<RailFolder, "id" | "servers">> | null): RailLayout {
  return layout.flatMap((e): RailLayout => {
    if (e.kind !== "folder" || e.folder.id !== id) return [e];
    if (!change) return e.folder.servers.map((s) => ({ kind: "server", id: s }));
    return [{ kind: "folder", folder: { ...e.folder, ...change, name: (change.name ?? e.folder.name).trim().slice(0, FOLDER_NAME_MAX) } }];
  });
}

/** Puts a server into a new folder of its own, where it is. */
export function folderOf(layout: RailLayout, server: string, newId: () => string = folderId): RailLayout {
  return layout.map((e) => (e.kind === "server" && e.id === server ? { kind: "folder", folder: { id: newId(), name: "", color: 0, servers: [server] } } : e));
}

/** A new folder's id: random, in the characters the instance takes. */
export function folderId() {
  const bytes = crypto.getRandomValues(new Uint8Array(9));
  return `f-${[...bytes].map((b) => b.toString(36).padStart(2, "0")).join("").slice(0, 14)}`;
}

/** `#rrggbb` for a folder color, or null for the theme's accent. */
export const folderHex = (color: number) => (color ? `#${color.toString(16).padStart(6, "0")}` : null);

/** The instance's `ServerRailItem`, as far as these need it. */
export type RailItem = {
  item:
    | { case: "serverId"; value: string }
    | { case: "folder"; value: { id: string; name: string; color: number; serverIds: string[] } }
    | { case: undefined; value?: undefined };
};

export const fromItems = (items: RailItem[]): RailLayout =>
  items.flatMap((i): RailLayout => {
    if (i.item.case === "serverId") return [{ kind: "server", id: i.item.value }];
    if (i.item.case === "folder") {
      const { id, name, color, serverIds } = i.item.value;
      return [{ kind: "folder", folder: { id, name, color, servers: [...serverIds] } }];
    }
    return [];
  });

export const toItems = (layout: RailLayout): RailItem[] =>
  layout.map((e) =>
    e.kind === "server"
      ? { item: { case: "serverId", value: e.id } }
      : { item: { case: "folder", value: { id: e.folder.id, name: e.folder.name, color: e.folder.color, serverIds: e.folder.servers } } },
  );

/** A folder's name as shown: its own, or its servers' names. */
export function folderLabel(folder: RailFolder, servers: Map<string, { name: string }>) {
  if (folder.name) return folder.name;
  const names = folder.servers.map((id) => servers.get(id)?.name ?? "").filter(Boolean);
  return names.length > 3 ? `${names.slice(0, 3).join(", ")} and ${names.length - 3} more` : names.join(", ") || "Folder";
}
