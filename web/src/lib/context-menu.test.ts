import assert from "node:assert/strict";
import { test } from "node:test";
import { drifted, extendMenu, items, joinSections, keyboardPoint, opensMenu, withExtensions, type MenuSection } from "./context-menu.ts";

const noop = () => {};
const ctx = { instanceKey: "fuwa.chat", serverId: "s", channel: {}, insert: noop } as never;
const base = (): MenuSection[] => [
  { id: "edit", items: [{ id: "paste", label: "Paste", onSelect: noop }] },
  { id: "insert", items: [] },
  { id: "danger", items: [{ id: "delete", label: "Delete", danger: true, onSelect: noop }] },
];
const ids = (sections: MenuSection[]) => sections.map((s) => `${s.id}:${s.items.map((i) => i.id).join(",")}`);

test("empty sections are left out", () => {
  assert.deepEqual(ids(withExtensions("composer", ctx, base())), ["edit:paste", "danger:delete"]);
});

test("features add items to a section by name, first or last", () => {
  const offA = extendMenu("composer", { section: "insert", build: () => [{ id: "poll", label: "Poll", onSelect: noop }] });
  const offB = extendMenu("composer", { section: "edit", at: "start", build: () => [{ id: "undo", label: "Undo", onSelect: noop }] });
  assert.deepEqual(ids(withExtensions("composer", ctx, base())), ["edit:undo,paste", "insert:poll", "danger:delete"]);
  offA();
  offB();
  assert.deepEqual(ids(withExtensions("composer", ctx, base())), ["edit:paste", "danger:delete"]);
});

test("a section nobody has yet goes before danger", () => {
  const off = extendMenu("composer", { section: "friends", build: () => [{ id: "add", label: "Add friend", onSelect: noop }] });
  assert.deepEqual(ids(withExtensions("composer", ctx, base())), ["edit:paste", "friends:add", "danger:delete"]);
  off();
});

test("a feature whose items throw doesn't break the menu", () => {
  const off = extendMenu("composer", {
    section: "edit",
    build: () => {
      throw new Error("broken");
    },
  });
  assert.deepEqual(ids(withExtensions("composer", ctx, base())), ["edit:paste", "danger:delete"]);
  off();
});

test("menus never change the sections they're given", () => {
  const given = base();
  const off = extendMenu("composer", { section: "edit", build: () => [{ id: "undo", label: "Undo", onSelect: noop }] });
  withExtensions("composer", ctx, given);
  off();
  assert.equal(given[0]!.items.length, 1);
});

test("items and joinSections drop what isn't there", () => {
  assert.deepEqual(
    items(false, null, undefined, { id: "a", label: "A", onSelect: noop }).map((i) => i.id),
    ["a"],
  );
  assert.deepEqual(ids(joinSections(base(), null, false, [{ id: "x", items: [] }])), ["edit:paste", "insert:", "danger:delete"].filter((s) => s !== "insert:"));
});

test("Shift+F10 and the Menu key open a menu; F10 alone and with Ctrl don't", () => {
  const key = (key: string, mods: Partial<Record<"shiftKey" | "ctrlKey" | "altKey" | "metaKey", boolean>> = {}) =>
    opensMenu({ key, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false, ...mods });
  assert.equal(key("F10", { shiftKey: true }), true);
  assert.equal(key("ContextMenu"), true);
  assert.equal(key("F10"), false);
  assert.equal(key("F10", { shiftKey: true, ctrlKey: true }), false);
  assert.equal(key("Enter", { shiftKey: true }), false);
});

test("a menu opened from the keyboard goes under the element, on screen", () => {
  const view = { width: 400, height: 300 };
  assert.deepEqual(keyboardPoint({ left: 20, bottom: 40, width: 200 }, view), { x: 36, y: 40 });
  assert.deepEqual(keyboardPoint({ left: 390, bottom: 320, width: 10 }, view), { x: 395, y: 300 });
  assert.deepEqual(keyboardPoint({ left: -50, bottom: -5, width: 20 }, view), { x: 0, y: 0 });
});

test("a finger that drifts more than a few pixels is scrolling, not holding", () => {
  assert.equal(drifted({ x: 0, y: 0 }, { x: 6, y: 6 }), false);
  assert.equal(drifted({ x: 0, y: 0 }, { x: 8, y: 8 }), true);
});
