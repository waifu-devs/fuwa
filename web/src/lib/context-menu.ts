import type { ComponentType, ReactNode } from "react";
import type { Conversation } from "@/gen/fuwa/v1/dm_pb";
import type { Channel, Member, Message, Server, User } from "@/gen/fuwa/v1/types_pb";

/*
 * Right-click menus. One menu draws at a time, wherever it was asked for
 * (components/ContextMenu.tsx); what's in it comes from the part of the app
 * that was clicked, as data: sections of items, each item doing what a button
 * on screen already does. Nothing here grants anything a button didn't.
 *
 * Features that land later (reactions, replies, pins, friends) add their items
 * with `extendMenu`, into a section by name, without touching the menus here.
 * docs/context-menus.md lists every menu and section, for the desktop app too.
 */

export type MenuIcon = ComponentType<{ className?: string }>;

type Base = { id: string; label: string; icon?: MenuIcon; disabled?: boolean };

/** Something to do. `hint` shows faded on the right, like where a link goes. */
export type MenuAction = Base & {
  kind?: "action";
  hint?: string;
  /** Red, for what removes or pushes someone out. */
  danger?: boolean;
  /** Stays open after, for picking several (roles). */
  keepOpen?: boolean;
  onSelect: () => void;
};

/** A choice that's on or off, like a role someone has. With `radio`, one of a set. */
export type MenuCheck = Base & {
  kind: "check";
  checked: boolean;
  radio?: boolean;
  /** A dot in this color before the label, like a role's. */
  color?: string;
  keepOpen?: boolean;
  onSelect: () => void;
};

/** More choices to the side. */
export type MenuSub = Base & { kind: "sub"; hint?: string; items: MenuEntry[] };

/** Faded text that explains, not a choice. */
export type MenuNote = { kind: "note"; id: string; label: string };

/** Anything drawn by its feature, like a row of emoji. `close` closes the menu. */
export type MenuCustom = { kind: "custom"; id: string; render: (close: () => void) => ReactNode };

export type MenuEntry = MenuAction | MenuCheck | MenuSub | MenuNote | MenuCustom;

/** Items that belong together; sections are drawn with a line between them. */
export type MenuSection = { id: string; items: MenuEntry[] };

/** What each menu is about, as features adding items to it see it. */
export interface MenuContexts {
  message: { instanceKey: string; serverId: string; channel: Channel; message: Message; mine: boolean };
  channel: { instanceKey: string; serverId: string; channel: Channel };
  category: { instanceKey: string; serverId: string; category: Channel };
  server: { instanceKey: string; server: Server };
  member: { instanceKey: string; serverId: string; user: User; member: Member | undefined };
  dm: { instanceKey: string; conversation: Conversation; other: User | undefined };
  /** A line in a direct message or secure channel, by its place there. */
  dm_message: { instanceKey: string; conversationId: string; seq: number; mine: boolean };
  composer: { instanceKey: string; serverId: string; channel: Channel; insert: (text: string) => void };
}

export type MenuKind = keyof MenuContexts;

export type MenuExtension<K extends MenuKind> = {
  /** The section it joins, by id; a new one is added before "danger" if the menu has none by that name. */
  section: string;
  /** First in the section, or last (the default). */
  at?: "start" | "end";
  build: (ctx: MenuContexts[K]) => MenuEntry[];
};

const extensions = new Map<MenuKind, MenuExtension<MenuKind>[]>();

/** Adds items to every menu of a kind. Returns a function that takes them out again. */
export function extendMenu<K extends MenuKind>(kind: K, extension: MenuExtension<K>): () => void {
  const list = extensions.get(kind) ?? [];
  const ext = extension as unknown as MenuExtension<MenuKind>;
  extensions.set(kind, [...list, ext]);
  return () => extensions.set(kind, (extensions.get(kind) ?? []).filter((e) => e !== ext));
}

/** A menu's own sections with what features added, empty sections left out. */
export function withExtensions<K extends MenuKind>(kind: K, ctx: MenuContexts[K], base: MenuSection[]): MenuSection[] {
  const sections = base.map((s) => ({ id: s.id, items: [...s.items] }));
  for (const ext of extensions.get(kind) ?? []) {
    let items: MenuEntry[];
    try {
      items = ext.build(ctx);
    } catch {
      // One feature's broken items don't take the menu down with them.
      continue;
    }
    if (!items.length) continue;
    let section = sections.find((s) => s.id === ext.section);
    if (!section) {
      section = { id: ext.section, items: [] };
      const danger = sections.findIndex((s) => s.id === "danger");
      sections.splice(danger === -1 ? sections.length : danger, 0, section);
    }
    if (ext.at === "start") section.items.unshift(...items);
    else section.items.push(...items);
  }
  return sections.filter((s) => s.items.length > 0);
}

/** Puts menus together in order, empty ones and empty sections left out. */
export const joinSections = (...groups: (MenuSection[] | null | undefined | false)[]): MenuSection[] =>
  groups.flatMap((g) => (g ? g.filter((s) => s.items.length > 0) : []));

/** Drops what's falsy, so a section can list items behind conditions. */
export const items = (...list: (MenuEntry | null | undefined | false)[]): MenuEntry[] => list.filter((e): e is MenuEntry => !!e);

// ───────────────────────── Where the menu opens ─────────────────────────

/** Where to put the menu: at the pointer, or under the focused element when opened from the keyboard. */
export type MenuPoint = { x: number; y: number };

/** The point a keyboard-opened menu goes to: the element's bottom left, kept on screen. */
export function keyboardPoint(rect: Pick<DOMRect, "left" | "bottom" | "width">, view: { width: number; height: number }): MenuPoint {
  const x = Math.min(Math.max(rect.left + Math.min(16, rect.width / 2), 0), view.width);
  const y = Math.min(Math.max(rect.bottom, 0), view.height);
  return { x, y };
}

/** How far a finger may drift while held before it counts as a scroll, not a press. */
export const HOLD_SLOP_PX = 10;
/** How long a finger is held before the menu opens. */
export const HOLD_MS = 450;

/** Whether a held finger has moved too far to count as holding still. */
export const drifted = (from: MenuPoint, to: MenuPoint) => Math.hypot(to.x - from.x, to.y - from.y) > HOLD_SLOP_PX;

/** The keys that open a menu from the keyboard, as in every desktop app: Shift+F10 and the Menu key. */
export const opensMenu = (e: Pick<KeyboardEvent, "key" | "shiftKey" | "ctrlKey" | "altKey" | "metaKey">) =>
  (e.key === "F10" && e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey) || e.key === "ContextMenu";
