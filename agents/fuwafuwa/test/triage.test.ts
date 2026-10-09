import assert from "node:assert/strict";
import { test } from "node:test";
import type { OpenIssue } from "../src/github.ts";
import { checkPlan, triage, type Plan, type TriageDeps } from "../src/triage.ts";

const open: OpenIssue[] = [{ number: 40, title: "Uploads fail on Firefox", body: "" }];
const group = (g: Partial<Plan["groups"][number]>): Plan["groups"][number] => ({
  items: [],
  action: "skip",
  existing_issue: null,
  title: null,
  body: null,
  reason: "",
  ...g,
});

test("a plan is checked before anything is filed", () => {
  const steps = checkPlan(
    {
      groups: [
        group({ items: [0, 1], action: "new_issue", title: " Dark mode ", body: "People want it." }),
        group({ items: [1, 2], action: "existing_issue", existing_issue: 40 }), // 1 is taken already
        group({ items: [3], action: "existing_issue", existing_issue: 99 }), // not an open issue
        group({ items: [4], action: "new_issue", title: "  " }), // no title
        group({ items: [5], action: "file_it" }), // not an action
        group({ items: [5, 7, -1, 1.5], action: "skip", reason: "praise" }), // 7 and up don't exist
      ],
    },
    6,
    open,
  );
  assert.deepEqual(steps, [
    { action: "new_issue", items: [0, 1], title: "Dark mode", body: "People want it." },
    { action: "existing_issue", items: [2], issue: 40 },
    { action: "skip", items: [5], reason: "praise" },
  ]);
});

function deps(plan: Plan, failCreate = false) {
  const marks: [string, string][] = [];
  const created: { title: string; body: string }[] = [];
  const comments: [number, string][] = [];
  const d: TriageDeps = {
    pending: async () => ["dark mode please", "a dark theme would be nice", "upload broke", "love it"].map((feedback, i) => ({ id: `p${i}`, feedback })),
    openIssues: async () => open,
    grouper: { group: async () => plan },
    createIssue: async (title, body) => {
      if (failCreate) throw new Error("GitHub answered 502 to POST /issues");
      created.push({ title, body });
      return { number: 41, url: "https://github.com/waifu-devs/fuwa/issues/41" };
    },
    comment: async (issue, body) => {
      comments.push([issue, body]);
      return { number: issue, url: `https://github.com/waifu-devs/fuwa/issues/${issue}#issuecomment-1` };
    },
    mark: async (p, outcome) => void marks.push([p.id, outcome]),
    instance: "fuwa.chat",
    log: () => {},
  };
  return { d, marks, created, comments };
}

test("a run files new issues, adds to open ones, and marks every post", async () => {
  const { d, marks, created, comments } = deps({
    groups: [
      group({ items: [0, 1], action: "new_issue", title: "Dark mode", body: "Two people asked for a dark theme." }),
      group({ items: [2], action: "existing_issue", existing_issue: 40 }),
      group({ items: [3], action: "skip", reason: "praise" }),
    ],
  });
  assert.deepEqual(await triage(d), { filed: 1, added: 1, skipped: 1, waiting: 0 });
  assert.equal(created[0]!.title, "Dark mode");
  assert.ok(created[0]!.body.includes("> dark mode please\n\n> a dark theme would be nice"));
  assert.equal(comments[0]![0], 40);
  assert.ok(comments[0]![1].includes("> upload broke"));
  assert.deepEqual(marks, [
    ["p0", "Filed as #41: https://github.com/waifu-devs/fuwa/issues/41"],
    ["p1", "Filed as #41: https://github.com/waifu-devs/fuwa/issues/41"],
    ["p2", "Added to #40: https://github.com/waifu-devs/fuwa/issues/40#issuecomment-1"],
    ["p3", "Not filed: praise"],
  ]);
});

test("what fails or isn't in the plan waits for the next run, unmarked", async () => {
  const { d, marks } = deps(
    { groups: [group({ items: [0, 1], action: "new_issue", title: "Dark mode" }), group({ items: [3], action: "skip", reason: "praise" })] },
    true,
  );
  assert.deepEqual(await triage(d), { filed: 0, added: 0, skipped: 1, waiting: 3 });
  assert.deepEqual(marks, [["p3", "Not filed: praise"]]);
});

test("nothing pending asks nobody", async () => {
  const { d } = deps({ groups: [] });
  d.pending = async () => [];
  d.grouper = { group: async () => assert.fail("asked Claude") };
  assert.deepEqual(await triage(d), { filed: 0, added: 0, skipped: 0, waiting: 0 });
});

test("what Claude writes reaches GitHub with no pings or live links", async () => {
  const { d, marks, created } = deps({
    groups: [
      group({
        items: [0, 1],
        action: "new_issue",
        title: "Dark mode @waifu-devs/core see https://evil.example",
        body: "cc @octocat ![x](https://evil.example/pixel.png) [docs](//evil.example) www.evil.example",
      }),
      group({ items: [3], action: "skip", reason: "ask @everyone at <@01JPERSON>" }),
    ],
  });
  d.pending = async () =>
    ["dark mode please http://evil.example", "a dark theme would be nice @someteam"].map((feedback, i) => ({ id: `p${i}`, feedback })).concat([
      { id: "p2", feedback: "upload broke" },
      { id: "p3", feedback: "love it" },
    ]);
  await triage(d);
  const { title, body } = created[0]!;
  for (const text of [title, body]) {
    assert.ok(!/@[\w-]/.test(text), text);
    assert.ok(!/https?:\/\/|\(\/\/|www\./i.test(text), text);
  }
  assert.ok(title.startsWith("Dark mode @\u200bwaifu-devs/core see hxxps[:]//evil.example"));
  assert.ok(body.includes("> dark mode please hxxp[:]//evil.example"));
  assert.equal(marks.at(-1)![1], "Not filed: ask @\u200beveryone at @\u200bsomeone");
});
