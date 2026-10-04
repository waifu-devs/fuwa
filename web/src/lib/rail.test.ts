import assert from "node:assert/strict";
import { test } from "node:test";
import { editFolder, folderId, folderOf, moveRail, railLayout, sameRail, stepRail, type RailLayout } from "./rail.ts";

const s = (id: string) => ({ kind: "server" as const, id });
const f = (id: string, servers: string[], name = "") => ({ kind: "folder" as const, folder: { id, name, color: 0, servers } });
const fixedId = () => "new";

test("servers you left drop out, new ones go last, folders left empty go", () => {
  const saved: RailLayout = [s("b"), f("g", ["gone", "c"]), f("empty", ["gone"]), s("gone"), s("a")];
  assert.deepEqual(railLayout(saved, ["a", "b", "c", "d"]), [s("b"), f("g", ["c"]), s("a"), s("d")]);
  assert.deepEqual(railLayout(null, ["a", "b"]), [s("a"), s("b")]);
  // A server placed twice keeps its first place.
  assert.deepEqual(railLayout([s("a"), f("g", ["a", "b"])], ["a", "b"]), [s("a"), f("g", ["b"])]);
});

test("servers move between top-level places and folders", () => {
  const layout: RailLayout = [s("a"), f("g", ["b", "c"]), s("d")];
  assert.deepEqual(moveRail(layout, { kind: "server", id: "d", folder: "", before: "a" }), [s("d"), s("a"), f("g", ["b", "c"])]);
  assert.deepEqual(moveRail(layout, { kind: "server", id: "a", folder: "g", before: "c" }), [f("g", ["b", "a", "c"]), s("d")]);
  assert.deepEqual(moveRail(layout, { kind: "server", id: "b", folder: "", before: null }), [s("a"), f("g", ["c"]), s("d"), s("b")]);
  // The last one out takes the folder with it.
  const lone: RailLayout = [f("g", ["b"]), s("a")];
  assert.deepEqual(moveRail(lone, { kind: "server", id: "b", folder: "", before: null }), [s("a"), s("b")]);
  // Dropping where it is changes nothing.
  assert.ok(sameRail(moveRail(layout, { kind: "server", id: "a", folder: "", before: "g" }), layout));
  assert.ok(sameRail(moveRail(layout, { kind: "server", id: "b", folder: "g", before: "c" }), layout));
});

test("dropping a server onto another makes a folder in its place", () => {
  const layout: RailLayout = [s("a"), s("b"), s("c")];
  assert.deepEqual(moveRail(layout, { kind: "combine", id: "c", with: "a" }, fixedId), [f("new", ["a", "c"]), s("b")]);
  assert.ok(sameRail(moveRail(layout, { kind: "combine", id: "a", with: "a" }, fixedId), layout));
});

test("folders move as a whole", () => {
  const layout: RailLayout = [s("a"), f("g", ["b"]), s("c")];
  assert.deepEqual(moveRail(layout, { kind: "folder", id: "g", before: "a" }), [f("g", ["b"]), s("a"), s("c")]);
  assert.deepEqual(moveRail(layout, { kind: "folder", id: "g", before: null }), [s("a"), s("c"), f("g", ["b"])]);
});

test("the keyboard steps into open folders and past closed ones", () => {
  const layout: RailLayout = [s("a"), f("g", ["b", "c"]), s("d")];
  const open = () => true;
  const closed = () => false;
  assert.deepEqual(stepRail(layout, "a", 1, open), [f("g", ["a", "b", "c"]), s("d")]);
  assert.deepEqual(stepRail(layout, "a", 1, closed), [f("g", ["b", "c"]), s("a"), s("d")]);
  assert.deepEqual(stepRail(layout, "d", -1, open), [s("a"), f("g", ["b", "c", "d"])]);
  assert.deepEqual(stepRail(layout, "b", -1, open), [s("a"), s("b"), f("g", ["c"]), s("d")]);
  assert.deepEqual(stepRail(layout, "c", 1, open), [s("a"), f("g", ["b"]), s("c"), s("d")]);
  assert.deepEqual(stepRail(layout, "b", 1, open), [s("a"), f("g", ["c", "b"]), s("d")]);
  assert.deepEqual(stepRail(layout, "g", 1, open), [s("a"), s("d"), f("g", ["b", "c"])]);
  assert.equal(stepRail(layout, "a", -1, open), null);
  assert.equal(stepRail(layout, "d", 1, open), null);
});

test("folders are renamed, recolored and dissolved in place", () => {
  const layout: RailLayout = [s("a"), f("g", ["b", "c"]), s("d")];
  assert.deepEqual(editFolder(layout, "g", { name: "  Games  ", color: 0xff66aa })[1], {
    kind: "folder",
    folder: { id: "g", name: "Games", color: 0xff66aa, servers: ["b", "c"] },
  });
  assert.deepEqual(editFolder(layout, "g", null), [s("a"), s("b"), s("c"), s("d")]);
  assert.deepEqual(folderOf(layout, "d", fixedId), [s("a"), f("g", ["b", "c"]), f("new", ["d"])]);
  assert.match(folderId(), /^f-[0-9a-z]{1,30}$/);
});
