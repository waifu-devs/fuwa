import assert from "node:assert/strict";
import { test } from "node:test";
import type { Member, Message } from "@/gen/fuwa/v1/types_pb";
import { byId, evicted, findMessage, memberById, messageIndex, sortMembers, touched, trimmed, withMember, type Loaded } from "./lists.ts";

const msg = (id: string) => ({ id }) as Message;
const loaded = (ids: string[], hasMore = false, loading = false): Loaded<Message> => ({ items: ids.map(msg), hasMore, loading });
const member = (id: string, name: string, nickname = "") => ({ user: { id, displayName: name, username: id }, nickname }) as unknown as Member;
const names = (list: Member[]) => list.map((m) => m.user!.id);

test("a message's place is found from the end first, then by halves", () => {
  const items = ["a", "c", "e", "g"].map(msg);
  assert.equal(messageIndex(items, "h"), 4);
  assert.equal(messageIndex(items, "a"), 0);
  assert.equal(messageIndex(items, "d"), 2);
  assert.equal(messageIndex(items, "e"), 2);
  assert.equal(messageIndex([], "a"), 0);
  // As the scan it replaces: the first id at or after it.
  for (const id of ["0", "a", "b", "c", "f", "g", "z"]) {
    const scan = items.findIndex((m) => m.id >= id);
    assert.equal(messageIndex(items, id), scan === -1 ? items.length : scan);
  }
  assert.equal(findMessage(items, "e")?.id, "e");
  assert.equal(findMessage(items, "d"), undefined);
});

test("trimming keeps the latest messages and says older ones exist", () => {
  const cut = trimmed(loaded(["1", "2", "3", "4", "5"]), 2);
  assert.deepEqual(cut.items.map((m) => m.id), ["4", "5"]);
  assert.equal(cut.hasMore, true);
  const short = loaded(["1"]);
  assert.equal(trimmed(short, 2), short);
  // A page on its way is left alone.
  const busy = loaded(["1", "2", "3"], false, true);
  assert.equal(trimmed(busy, 1), busy);
});

test("places left lately keep a page, older ones let go, the open ones stay whole", () => {
  const messages = {
    a: loaded(["1", "2", "3"]),
    b: loaded(["1", "2", "3"]),
    c: loaded(["1", "2", "3"]),
    d: loaded(["1", "2", "3"]),
    untracked: loaded(["1", "2", "3"]),
  };
  const recent = touched(["b", "c", "d"], ["a", null]);
  assert.deepEqual(recent, ["a", "b", "c", "d"]);
  const next = evicted(messages, recent, new Set(["a"]), 2, 1);
  assert.equal(next.a, messages.a);
  assert.deepEqual(next.b!.items.map((m) => m.id), ["3"]);
  assert.equal(next.b!.hasMore, true);
  assert.deepEqual(next.c!.items.map((m) => m.id), ["3"]);
  assert.equal(next.d, undefined);
  // Never opened through focus: not ours to drop.
  assert.equal(next.untracked, messages.untracked);
  // Nothing to do: the same record back.
  assert.equal(evicted(next, recent, new Set(["a"]), 2, 1), next);
});

test("members sort by name, and one changing moves only them", () => {
  const list = sortMembers([member("3", "Cleo"), member("1", "ana"), member("2", "Bo")]);
  assert.deepEqual(names(list), ["1", "2", "3"]);
  // A new name moves them where a sort would.
  const renamed = withMember(list, member("1", "Zed"));
  assert.deepEqual(names(renamed), ["2", "3", "1"]);
  assert.deepEqual(names(renamed), names(sortMembers([...list.filter((m) => m.user!.id !== "1"), member("1", "Zed")])));
  // A newcomer goes after namesakes, as the stable sort put them.
  const joined = withMember(list, member("4", "Bo"));
  assert.deepEqual(names(joined), ["1", "2", "4", "3"]);
  // Nicknames come first.
  assert.deepEqual(names(withMember(list, member("3", "Cleo", "Aaron"))), ["3", "1", "2"]);
  assert.equal(list.length, 3);
});

test("lookups by id follow the list they were made for", () => {
  const list = [member("1", "a"), member("2", "b")];
  assert.equal(memberById(list, "2"), list[1]);
  assert.equal(memberById(list, "3"), undefined);
  const next = [...list, member("3", "c")];
  assert.equal(memberById(next, "3"), next[2]);
  const servers = [{ id: "s1", name: "one" }];
  assert.equal(byId(servers, "s1")?.name, "one");
  assert.equal(byId(servers, undefined), undefined);
});
