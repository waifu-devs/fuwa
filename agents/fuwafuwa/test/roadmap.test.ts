import assert from "node:assert/strict";
import { test } from "node:test";
import {
  branchFor,
  captionOf,
  doingColumn,
  failedPost,
  mergedPost,
  parseRepos,
  prIn,
  prNumber,
  repoFor,
  reviewPost,
  setupCommand,
  shotPost,
  shotsIn,
  shouldStart,
  startPost,
  stateOf,
  summaryOf,
  taskPrompt,
  todoColumn,
  type Post,
} from "../src/roadmap.ts";

const columns = [
  { id: "c1", name: "To-Do", done: false },
  { id: "c2", name: "In Progress", done: false },
  { id: "c3", name: "Done", done: true },
];

test("columns are found by their usual names, or the ones set", () => {
  assert.equal(todoColumn(columns)?.id, "c1");
  assert.equal(doingColumn(columns)?.id, "c2");
  assert.equal(todoColumn(columns, "done")?.id, "c3");
  assert.equal(doingColumn([{ id: "x", name: "Review", done: false }]), undefined);
});

test("a card goes to the repository one of its labels names, or the first", () => {
  const repos = parseRepos("waifu-devs/fuwa, waifu-devs/site");
  assert.deepEqual(repos, ["waifu-devs/fuwa", "waifu-devs/site"]);
  assert.equal(repoFor(["Approved", "Site"], repos), "waifu-devs/site");
  assert.equal(repoFor(["Approved"], repos), "waifu-devs/fuwa");
  assert.deepEqual(parseRepos(undefined), ["waifu-devs/fuwa"]);
  assert.throws(() => parseRepos("fuwa"));
  assert.throws(() => parseRepos("a/b; rm -rf /"));
});

test("a branch is the title in a few words and the end of the card's id", () => {
  assert.equal(branchFor({ id: "01HXYZABC123", title: "Dark mode for the émoji picker!" }), "roadmap/dark-mode-for-the-emoji-picker-abc123");
  assert.equal(branchFor({ id: "42", title: "🎉🎉" }), "roadmap/card-42");
  assert.ok(branchFor({ id: "1", title: "x".repeat(200) }).length < 60);
});

test("the task says what to build, where, and how to report", () => {
  const p = taskPrompt({ title: "Dark mode", body: "Make it dark.", repo: "waifu-devs/fuwa", dir: "/home/user/fuwa", branch: "roadmap/dark-mode-abc123" });
  assert.match(p, /waifu-devs\/fuwa, which is checked out at \/home\/user\/fuwa/);
  assert.match(p, /<card>\n# Dark mode\n\nMake it dark.\n<\/card>/);
  assert.match(p, /branch named roadmap\/dark-mode-abc123/);
  assert.match(p, /\/home\/user\/roadmap-shots\//);
  assert.match(p, /Pull request: <its URL>/);
  assert.match(taskPrompt({ title: "T", body: " ", repo: "a/b", dir: "/d", branch: "b" }), /no description/);
});

test("the setup command quotes what it's given", () => {
  const cmd = setupCommand("waifu-devs/fuwa", "/home/user/fuwa");
  assert.match(cmd, /gh repo clone 'waifu-devs\/fuwa' '\/home\/user\/fuwa'/);
  assert.match(cmd, /mkdir -p '\/home\/user\/roadmap-shots'/);
});

test("screenshots are read from a listing, and captioned from their names", () => {
  assert.deepEqual(shotsIn("02-after.png\nnotes.txt\n01-before.PNG\n../x.png\nbad name.png\n03-x.webp\n"), [
    "01-before.PNG",
    "02-after.png",
    "03-x.webp",
  ]);
  assert.equal(captionOf("03-empty_state.png"), "Empty state");
  assert.equal(captionOf("01.png"), "Screenshot");
  assert.equal(shotPost("02-dark-mode-on.png"), "📸 Dark mode on");
});

test("the pull request is the last link in the card's repository", () => {
  const text = "See https://github.com/waifu-devs/site/pull/3 and\nPull request: https://github.com/waifu-devs/fuwa/pull/412";
  assert.equal(prIn(text, "waifu-devs/fuwa"), "https://github.com/waifu-devs/fuwa/pull/412");
  assert.equal(prIn(text, "waifu-devs/other"), "");
  assert.equal(prNumber("https://github.com/waifu-devs/fuwa/pull/412"), 412);
});

test("a summary drops the pull request line, pings, and length", () => {
  assert.equal(summaryOf("Did it @everyone <@123>\nPull request: https://x"), "Did it @​everyone @someone");
  assert.equal(summaryOf("x".repeat(50), 10).length, 10);
});

const post = (id: string, at: number, content: string, files: string[] = []): Post => ({ id, at, content, files });

test("the agent's posts read back into where a card is", () => {
  assert.equal(stateOf([]).phase, "none");
  const start = post("p1", 1, startPost("waifu-devs/fuwa", "roadmap/x", { id: "bx_abcdefgh", promptId: "prompt_1", conversationId: "c-1" }));
  let st = stateOf([start, post("p2", 2, shotPost("01-a.png"), ["01-a.png"])]);
  assert.equal(st.phase, "working");
  assert.equal(st.sandboxId, "bx_abcdefgh");
  assert.equal(st.promptId, "prompt_1");
  assert.equal(st.conversationId, "c-1");
  assert.equal(st.repo, "waifu-devs/fuwa");
  assert.equal(st.startPost, "p1");
  assert.deepEqual([...st.posted], ["01-a.png"]);

  const review = post("p3", 3, reviewPost("Did it.", "https://github.com/waifu-devs/fuwa/pull/9"));
  st = stateOf([review, start]); // any order
  assert.equal(st.phase, "review");
  assert.equal(st.pr, "https://github.com/waifu-devs/fuwa/pull/9");
  assert.equal(stateOf([start, review, post("p4", 4, mergedPost(st.pr))]).phase, "merged");

  // A start whose sandbox isn't up yet.
  st = stateOf([post("p1", 1, startPost("waifu-devs/fuwa", "roadmap/x"))]);
  assert.equal(st.phase, "working");
  assert.equal(st.sandboxId, "");

  // A failure, then a new try.
  const failed = post("p5", 5, failedPost("no PR"));
  assert.equal(stateOf([start, failed]).phase, "failed");
  st = stateOf([start, failed, post("p6", 6, startPost("waifu-devs/site", "roadmap/x", { id: "bx_zzzzzzzz" }))]);
  assert.equal(st.phase, "working");
  assert.equal(st.sandboxId, "bx_zzzzzzzz");
  assert.equal(st.repo, "waifu-devs/site");
  assert.equal(st.promptId, "");
});

test("a card starts once, and again only if it failed and moved since", () => {
  assert.equal(shouldStart(stateOf([]), 0), true);
  const failed = stateOf([post("p", 100, failedPost("x"))]);
  assert.equal(shouldStart(failed, 50), false);
  assert.equal(shouldStart(failed, 150), true);
  const review = stateOf([
    post("a", 1, startPost("a/b", "x", { id: "bx_abcdefgh" })),
    post("b", 2, reviewPost("", "https://github.com/a/b/pull/1")),
  ]);
  assert.equal(shouldStart(review, 999), false);
});
