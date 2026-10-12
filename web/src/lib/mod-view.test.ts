import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { Channel, Message, ModCase, ModNote } from "@/gen/fuwa/v1/types_pb";
import {
  caseMatches,
  daysSince,
  DISMISSED,
  isTab,
  kindLabel,
  mayReport,
  MEMBER_REPORT,
  OPEN,
  reasonLabel,
  REASONS,
  RESOLVED,
  snippet,
  statusLabel,
  statusOfTab,
  TABS,
  withCase,
  withNote,
} from "./mod-view.ts";

const chattools = JSON.parse(readFileSync(new URL("../../../locales/en/chattools.json", import.meta.url), "utf8")) as Record<string, unknown>;
const es = JSON.parse(readFileSync(new URL("../../../locales/es/chattools.json", import.meta.url), "utf8")) as Record<string, unknown>;

const mc = (id: string, status = OPEN, subjectId = "U1"): ModCase => ({ id, status, subjectId }) as ModCase;
const ids = (list: ModCase[]) => list.map((c) => c.id);

test("every report reason the protocol has, once, with words in both languages", () => {
  assert.deepEqual(
    REASONS.map((r) => r.value),
    [1, 2, 3, 4, 5, 6, 7, 8, 9],
  );
  for (const r of REASONS) {
    for (const key of [r.label, r.about]) {
      const short = key.replace(/^chattools\./, "");
      assert.equal(typeof chattools[short], "string", key);
      assert.equal(typeof es[short], "string", `es ${key}`);
    }
  }
  assert.equal(reasonLabel(2), "chattools.modview.reason.harassment");
  // One a newer instance adds reads as "something else".
  assert.equal(reasonLabel(42), "chattools.modview.reason.other");
});

test("kinds, states and tabs read in words the catalog has", () => {
  for (const key of [kindLabel(1), kindLabel(2), kindLabel(MEMBER_REPORT), statusLabel(OPEN), statusLabel(RESOLVED), statusLabel(DISMISSED)]) {
    assert.equal(typeof chattools[key.replace(/^chattools\./, "")], "string", key);
  }
  assert.deepEqual(
    TABS.map(statusOfTab),
    [OPEN, RESOLVED, DISMISSED, 0],
  );
  assert.ok(isTab("open"));
  assert.ok(!isTab("person"));
});

test("a filter takes a state and a person", () => {
  assert.ok(caseMatches(mc("A"), { status: 0, subjectId: "" }));
  assert.ok(caseMatches(mc("A"), { status: OPEN, subjectId: "U1" }));
  assert.ok(!caseMatches(mc("A", RESOLVED), { status: OPEN, subjectId: "" }));
  assert.ok(!caseMatches(mc("A"), { status: 0, subjectId: "U2" }));
});

test("a changed case is replaced, dropped or put in its place", () => {
  const open = { status: OPEN, subjectId: "" };
  const list = [mc("C"), mc("B"), mc("A")];
  // Reported again: replaced where it is.
  const again = { ...mc("B"), reportCount: 2 } as ModCase;
  const replaced = withCase(list, again, open, false);
  assert.deepEqual(ids(replaced), ["C", "B", "A"]);
  assert.equal(replaced[1], again);
  // Resolved: off the Open tab.
  assert.deepEqual(ids(withCase(list, mc("B", RESOLVED), open, false)), ["C", "A"]);
  // A new one goes first.
  assert.deepEqual(ids(withCase(list, mc("D"), open, false)), ["D", "C", "B", "A"]);
  // One that doesn't belong isn't added.
  assert.deepEqual(ids(withCase(list, mc("D", DISMISSED), open, false)), ["C", "B", "A"]);
  // Reopened inside what's loaded: in its place.
  const gappy = [mc("E"), mc("C"), mc("A")];
  assert.deepEqual(ids(withCase(gappy, mc("B"), open, false)), ["E", "C", "B", "A"]);
  // Older than every case loaded: waits for its page, unless there are no more pages.
  assert.deepEqual(ids(withCase([mc("C"), mc("B")], mc("A"), open, false)), ["C", "B"]);
  assert.deepEqual(ids(withCase([mc("C"), mc("B")], mc("A"), open, true)), ["C", "B", "A"]);
  // Someone else's case stays out of one person's list.
  assert.deepEqual(ids(withCase(list, mc("D", OPEN, "U2"), { status: 0, subjectId: "U1" }, true)), ["C", "B", "A"]);
});

test("notes are added once, newest first", () => {
  const n = (id: string) => ({ id }) as ModNote;
  assert.deepEqual(
    withNote([n("B"), n("A")], n("C")).map((x) => x.id),
    ["C", "B", "A"],
  );
  assert.deepEqual(
    withNote([n("B"), n("A")], n("B")).map((x) => x.id),
    ["B", "A"],
  );
});

test("only others' ordinary messages in channels the server reads can be reported", () => {
  const text = { type: 1, shared: undefined } as unknown as Channel;
  const msg = (authorId: string, kind = 0) => ({ authorId, kind }) as Message;
  assert.ok(mayReport(text, msg("U2"), "U1"));
  assert.ok(!mayReport(text, msg("U1"), "U1"), "your own");
  assert.ok(!mayReport(text, msg("U2", 1), "U1"), "a join line");
  assert.ok(!mayReport(text, msg(""), "U1"), "no author");
  assert.ok(!mayReport(text, msg("U2"), undefined), "signed out");
  assert.ok(!mayReport(undefined, msg("U2"), "U1"), "no channel");
  assert.ok(!mayReport({ type: 6 } as Channel, msg("U2"), "U1"), "secure");
  assert.ok(!mayReport({ type: 1, shared: { home: false } } as unknown as Channel, msg("U2"), "U1"), "shared from elsewhere");
  assert.ok(mayReport({ type: 1, shared: { home: true } } as unknown as Channel, msg("U2"), "U1"), "shared from here");
});

test("snippets are one line and cut", () => {
  assert.equal(snippet("  hello\n\nthere  "), "hello there");
  const long = snippet("a".repeat(200), 10);
  assert.equal(long.length, 10);
  assert.ok(long.endsWith("…"));
  assert.equal(daysSince(new Date(0), 3 * 86_400_000 + 5), 3);
  assert.equal(daysSince(new Date(10_000), 0), 0);
});
