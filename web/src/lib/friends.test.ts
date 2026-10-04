import assert from "node:assert/strict";
import { test } from "node:test";
import type { Friend, FriendEvent } from "@/gen/fuwa/v1/friend_pb";
import { applyFriendEvent, BLOCKED, cleanUsername, FRIEND, inTab, INCOMING, OUTGOING, pendingLine, stateWith, waitingForYou } from "./friends.ts";

// Run with `pnpm test` (node's own test runner; no extra dependencies).

const DAY = 86_400_000;
const at = (ms: number) => ({ seconds: BigInt(Math.floor(ms / 1000)), nanos: 0 }) as never;
const friend = (id: string, name: string, state: number, extra: Partial<Friend> = {}): Friend =>
  ({ user: { id, username: name, displayName: "" }, state, online: false, ...extra }) as never;
const event = (payload: FriendEvent["payload"]): FriendEvent => ({ payload }) as never;

test("events add, replace, sort and remove people, and arriving twice changes nothing", () => {
  let list: Friend[] = [];
  list = applyFriendEvent(list, event({ case: "changed", value: friend("b", "bea", INCOMING) }));
  list = applyFriendEvent(list, event({ case: "changed", value: friend("a", "ami", FRIEND) }));
  assert.deepEqual(list.map((f) => f.user?.username), ["ami", "bea"]);
  const again = applyFriendEvent(list, event({ case: "changed", value: friend("b", "bea", FRIEND) }));
  assert.deepEqual(again.map((f) => f.state), [FRIEND, FRIEND]);
  const gone = applyFriendEvent(again, event({ case: "removed", value: "a" }));
  assert.deepEqual(gone.map((f) => f.user?.id), ["b"]);
  assert.equal(applyFriendEvent(gone, event({ case: "removed", value: "a" })), gone);
});

test("presence reaches friends only, and the same news twice keeps the list", () => {
  const list = [friend("a", "ami", FRIEND), friend("b", "bea", OUTGOING)];
  const online = applyFriendEvent(list, event({ case: "presence", value: { userId: "a", online: true } as never }));
  assert.equal(online[0]!.online, true);
  assert.equal(applyFriendEvent(online, event({ case: "presence", value: { userId: "a", online: true } as never })), online);
  assert.equal(applyFriendEvent(list, event({ case: "presence", value: { userId: "b", online: true } as never })), list);
});

test("tabs sort people out, and a search narrows them", () => {
  const now = 10 * DAY;
  const list = [
    friend("a", "ami", FRIEND, { online: true }),
    friend("b", "bea", FRIEND),
    friend("c", "cho", INCOMING, { expiresAt: at(now + DAY) }),
    friend("d", "dax", OUTGOING, { expiresAt: at(now - 1) }),
    friend("e", "eli", BLOCKED),
  ];
  const ids = (tab: Parameters<typeof inTab>[1], q = "") => inTab(list, tab, q, now).map((f) => f.user?.id);
  assert.deepEqual(ids("online"), ["a"]);
  assert.deepEqual(ids("all"), ["a", "b"]);
  assert.deepEqual(ids("pending"), ["c"]); // dax's ran out
  assert.deepEqual(ids("blocked"), ["e"]);
  assert.deepEqual(ids("all", "@BE"), ["b"]);
  assert.equal(waitingForYou(list, now), 1);
  assert.equal(stateWith(list, "d", now), 0);
  assert.equal(stateWith(list, "e", now), BLOCKED);
  assert.equal(pendingLine(list[2]!, now), "Wants to be friends · 1 day left");
});

test("usernames are read as typed", () => {
  assert.equal(cleanUsername("  @Mika "), "mika");
  assert.equal(cleanUsername("@@rin"), "rin");
});
