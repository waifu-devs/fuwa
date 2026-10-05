import assert from "node:assert/strict";
import { test } from "node:test";
import {
  activeAccount,
  loadSaved,
  savedInstance,
  updateSaved,
  withAccount,
  withActive,
  withCard,
  withoutAccount,
  withoutInstance,
  ownPicture,
  replacedTokens,
  type Card,
  type SavedInstance,
} from "./saved.ts";

function memory(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    data,
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => void data.set(k, v),
    removeItem: (k: string) => void data.delete(k),
  };
}

const URL_A = "https://fuwa.chat";
const card = (userId: string, name = userId): Card => ({ userId, username: name, displayName: name.toUpperCase(), avatarUrl: "" });

test("a list from before several accounts keeps its sessions, and is replaced once written", () => {
  const at = memory({ "fuwa:instances:v1": JSON.stringify([{ url: URL_A, token: "t1" }, { url: "https://other.chat", token: null }]) });
  const list = loadSaved(at);
  assert.equal(activeAccount(list[0])?.token, "t1");
  assert.equal(list[1]!.active, null);
  assert.deepEqual(list[1]!.accounts, []);
  updateSaved((l) => withAccount(l, "https://other.chat", "t2", card("u2")), at);
  assert.equal(at.getItem("fuwa:instances:v1"), null, "the old list goes once the new one is written");
  assert.equal(activeAccount(savedInstance(loadSaved(at), URL_A))?.token, "t1");
});

test("signing in to a second account on an instance keeps the first and makes the new one active", () => {
  let list: SavedInstance[] = withAccount([], URL_A, "t1", card("u1"));
  list = withAccount(list, URL_A, "t2", card("u2"));
  const inst = savedInstance(list, URL_A)!;
  assert.deepEqual(inst.accounts.map((a) => a.userId), ["u1", "u2"]);
  assert.equal(inst.active, "u2");
  assert.equal(list.length, 1, "the same instance, however its address is written");
  assert.equal(withAccount(list, "https://fuwa.chat/", "t3", card("u3")).length, 1);
});

test("signing in again to a kept account replaces its session instead of adding it twice", () => {
  let list = withAccount(withAccount([], URL_A, "t1", card("u1")), URL_A, "t2", card("u2"));
  list = withAccount(list, URL_A, "t1b", card("u1"));
  const inst = savedInstance(list, URL_A)!;
  assert.deepEqual(inst.accounts.map((a) => [a.userId, a.token]), [["u2", "t2"], ["u1", "t1b"]]);
  assert.equal(inst.active, "u1");
});

test("an old session learns whose it is, once, and says so", () => {
  const at = memory({ "fuwa:instances:v1": JSON.stringify([{ url: URL_A, token: "t1" }]) });
  const first = withCard(loadSaved(at), URL_A, "t1", card("u1", "mika"));
  assert.equal(first.wasUnknown, true);
  const inst = savedInstance(first.list, URL_A)!;
  assert.equal(inst.active, "u1");
  assert.equal(inst.accounts[0]!.username, "mika");
  const again = withCard(first.list, URL_A, "t1", card("u1", "mika"));
  assert.equal(again.list, first.list, "nothing to write when the card is the same");
  assert.equal(again.wasUnknown, false);
  const renamed = withCard(first.list, URL_A, "t1", card("u1", "mika2"));
  assert.equal(renamed.wasUnknown, false);
  assert.equal(savedInstance(renamed.list, URL_A)!.accounts[0]!.username, "mika2");
});

test("an old session that turns out to be an account already kept leaves one entry, with the session in use", () => {
  let list = withAccount([], URL_A, "t2", card("u1"));
  list = [{ ...list[0]!, accounts: [...list[0]!.accounts, { userId: "", token: "old", username: "", displayName: "", avatarUrl: "" }], active: "" }];
  const { list: next } = withCard(list, URL_A, "old", card("u1"));
  const inst = savedInstance(next, URL_A)!;
  assert.deepEqual(inst.accounts.map((a) => [a.userId, a.token]), [["u1", "old"]]);
  assert.equal(inst.active, "u1");
});

test("switching only goes to an account that's kept", () => {
  const list = withAccount(withAccount([], URL_A, "t1", card("u1")), URL_A, "t2", card("u2"));
  assert.equal(savedInstance(withActive(list, URL_A, "u1"), URL_A)!.active, "u1");
  assert.equal(withActive(list, URL_A, "nobody"), list);
});

test("forgetting one account leaves the others kept and the instance signed out when it was active", () => {
  const list = withAccount(withAccount([], URL_A, "t1", card("u1")), URL_A, "t2", card("u2"));
  const inst = savedInstance(withoutAccount(list, URL_A, "u2"), URL_A)!;
  assert.deepEqual(inst.accounts.map((a) => a.userId), ["u1"]);
  assert.equal(inst.active, null);
  const other = savedInstance(withoutAccount(list, URL_A, "u1"), URL_A)!;
  assert.equal(other.active, "u2", "forgetting one that isn't active leaves the active one");
  assert.deepEqual(withoutInstance(list, URL_A), []);
});

test("a change reads the kept list again, so another tab's account isn't lost", () => {
  const at = memory();
  updateSaved((l) => withAccount(l, URL_A, "t1", card("u1")), at);
  const stale = loadSaved(at);
  // Another tab adds an account meanwhile.
  updateSaved((l) => withAccount(l, URL_A, "t2", card("u2")), at);
  // This tab's next change goes from what's kept now, not its stale copy.
  updateSaved((l) => withActive(l, URL_A, "u1"), at);
  assert.equal(savedInstance(stale, URL_A)!.accounts.length, 1);
  assert.deepEqual(savedInstance(loadSaved(at), URL_A)!.accounts.map((a) => a.userId), ["u1", "u2"]);
});

test("a kept list with broken entries keeps what it can", () => {
  const at = memory({
    "fuwa:accounts:v2": JSON.stringify([
      { url: URL_A, active: "u9", accounts: [{ userId: "u1", token: "t1", username: "a", displayName: "A", avatarUrl: "" }, { userId: "u2" }] },
      { nope: true },
    ]),
  });
  const list = loadSaved(at);
  assert.equal(list.length, 1);
  assert.deepEqual(list[0]!.accounts.map((a) => a.userId), ["u1"]);
  assert.equal(list[0]!.active, null, "an active account that isn't kept is nobody");
});

test("a session is only ever kept under its own account, and a replaced one is gone and named to be ended", () => {
  const at = memory();
  updateSaved((l) => withAccount(l, URL_A, "token-one", card("u1")), at);
  updateSaved((l) => withAccount(l, URL_A, "token-two", card("u2")), at);
  let before: SavedInstance[] = [];
  const after = updateSaved((l) => {
    before = l;
    return withAccount(l, URL_A, "token-one-again", card("u1"));
  }, at);
  assert.deepEqual(replacedTokens(before, after, URL_A), ["token-one"]);
  const raw = at.getItem("fuwa:accounts:v2")!;
  assert.ok(!raw.includes('"token-one"'), "the replaced session isn't kept anywhere");
  const owners = savedInstance(loadSaved(at), URL_A)!.accounts.map((a) => [a.token, a.userId]);
  assert.deepEqual(owners.sort(), [["token-one-again", "u1"], ["token-two", "u2"]]);
});

test("a kept picture is only one on the instance itself", () => {
  const list = withAccount([], URL_A, "t1", { ...card("u1"), avatarUrl: "https://tracker.example/p.png" });
  assert.equal(savedInstance(list, URL_A)!.accounts[0]!.avatarUrl, "");
  assert.equal(ownPicture("https://fuwa.chat/media/a.png", URL_A), "https://fuwa.chat/media/a.png");
  assert.equal(ownPicture("/media/a.png", URL_A), "/media/a.png");
  assert.equal(ownPicture("//tracker.example/p.png", URL_A), "");
  assert.equal(ownPicture("https://fuwa.chat.tracker.example/p.png", URL_A), "");
});
