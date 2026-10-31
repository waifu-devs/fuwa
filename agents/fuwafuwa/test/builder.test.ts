import assert from "node:assert/strict";
import { test } from "node:test";
import type { BoatEvent, CommandResult, NewSandbox, PromptRun } from "../src/boat.ts";
import { Builder, lastResponse, type BoardPort, type BoardView, type BuilderOptions, type CardInfo, type ShotFile } from "../src/builder.ts";
import { MergeRefused } from "../src/github.ts";
import { startPost, type Post } from "../src/roadmap.ts";

const PR = "https://github.com/waifu-devs/fuwa/pull/77";

/** A board in memory, with the agent's posts per card. */
class FakeBoard implements BoardPort {
  view: BoardView = {
    columns: [
      { id: "todo", name: "To do", done: false },
      { id: "doing", name: "Doing", done: false },
      { id: "done", name: "Done", done: true },
    ],
    labels: [
      { id: "ok", name: "Approved" },
      { id: "site", name: "site" },
    ],
    cards: [],
    bodies: new Map(),
  };
  threads = new Map<string, Post[]>();
  files: ShotFile[] = [];
  clock = 1000;
  async read() {
    return structuredClone(this.view);
  }
  async move(cardId: string, columnId: string) {
    const card = this.view.cards.find((c) => c.id === cardId)!;
    card.columnId = columnId;
    card.updatedAt = ++this.clock;
  }
  async posts(cardId: string) {
    return this.threads.get(cardId) ?? [];
  }
  async post(cardId: string, content: string, file?: ShotFile) {
    const list = this.threads.get(cardId) ?? [];
    const id = `m${list.length + 1}-${cardId}`;
    list.push({ id, content, at: ++this.clock, files: file ? [file.name] : [] });
    this.threads.set(cardId, list);
    if (file) this.files.push(file);
    return id;
  }
  async edit(postId: string, content: string) {
    for (const list of this.threads.values()) for (const p of list) if (p.id === postId) p.content = content;
  }
  texts(cardId: string) {
    return (this.threads.get(cardId) ?? []).map((p) => p.content);
  }
}

class FakeBoat {
  created: NewSandbox[] = [];
  prompts: string[] = [];
  commands: string[] = [];
  stopped: string[] = [];
  deleted: string[] = [];
  shots: string[] = [];
  /** How many looks before the run is done, and how it ends. */
  runningFor = 2;
  status: PromptRun["status"] = "finished";
  answer = `Added dark mode.\nPull request: ${PR}`;
  setupExit = 0;
  #n = 0;
  async create(opts: NewSandbox) {
    this.created.push(opts);
    return { id: `bx_${"abcdefgh".slice(0, 7)}${this.created.length + 1}`, state: "provisioning" };
  }
  async waitReady(id: string) {
    return { id, state: "ready" };
  }
  async run(_id: string, command: string): Promise<CommandResult> {
    this.commands.push(command);
    if (command.startsWith("ls ")) return { exitCode: 0, stdout: this.shots.join("\n"), stderr: "" };
    return { exitCode: this.setupExit, stdout: "", stderr: "" };
  }
  async prompt(_id: string, prompt: string) {
    this.prompts.push(prompt);
    return { promptId: "prompt_1", conversationId: "convo-1", status: "queued" as const, done: false };
  }
  async promptRun(): Promise<PromptRun> {
    const done = ++this.#n > this.runningFor;
    if (this.#n === 1) this.shots.push("01-start.png");
    if (done) this.shots.push("02-finished.png");
    return { promptId: "prompt_1", conversationId: "convo-1", status: done ? this.status : "running", done };
  }
  async events(): Promise<BoatEvent[]> {
    return [
      { type: "response", taskId: "prompt_1", data: { content: "partial", is_streaming: true } },
      { type: "response", taskId: "prompt_1", data: { content: this.answer } },
    ];
  }
  async interrupt() {}
  async download() {
    return new Uint8Array([137, 80, 78, 71]);
  }
  async stop(id: string) {
    this.stopped.push(id);
  }
  async delete(id: string) {
    this.deleted.push(id);
  }
}

class FakePulls {
  merged: number[] = [];
  refuse = "";
  state: "open" | "closed" = "open";
  async get(_repo: string, number: number) {
    return { number, url: PR, state: this.state, merged: this.merged.includes(number) };
  }
  async merge(_repo: string, number: number) {
    if (this.refuse) throw new MergeRefused(this.refuse);
    this.merged.push(number);
  }
}

function card(id: string, over: Partial<CardInfo> = {}): CardInfo {
  return { id, title: `Card ${id}`, columnId: "todo", labelIds: ["ok"], updatedAt: 1, ...over };
}

function setup(over: Partial<BuilderOptions> = {}) {
  const board = new FakeBoard();
  const boat = new FakeBoat();
  const pulls = new FakePulls();
  const logs: string[] = [];
  const builder = new Builder({
    board,
    boat,
    pulls,
    repos: ["waifu-devs/fuwa", "waifu-devs/site"],
    label: "Approved",
    maxSandboxes: 2,
    sandbox: { ttlSeconds: 7200, environment: "base" },
    pollMs: 0,
    maxMs: 60 * 60_000,
    mergeMethod: "squash",
    log: (l) => logs.push(l),
    sleep: () => new Promise((r) => setImmediate(r)),
    now: () => board.clock,
    ...over,
  });
  return { board, boat, pulls, builder, logs };
}

async function until(what: () => boolean) {
  for (let i = 0; i < 2000 && !what(); i++) await new Promise((r) => setImmediate(r));
  assert.ok(what(), "it never happened");
}

test("an approved card in To do is built, posted and merged from Done", async () => {
  const { board, boat, pulls, builder } = setup();
  board.view.cards = [card("a"), card("b", { labelIds: [] }), card("c", { columnId: "doing" })];
  board.view.bodies.set("a", "Make it dark.");
  await builder.sweep();
  await until(() => builder.active === 0 && board.texts("a").some((t) => t.startsWith("✅")));

  // Only the approved card in To do; it moved to Doing.
  assert.equal(boat.created.length, 1);
  assert.equal(boat.created[0]!.environment, "base");
  assert.deepEqual(boat.created[0]!.labels, { "fuwa-card": "a" });
  assert.equal(board.view.cards[0]!.columnId, "doing");
  assert.match(boat.commands[0]!, /gh repo clone 'waifu-devs\/fuwa'/);
  assert.match(boat.prompts[0]!, /Make it dark\./);

  const texts = board.texts("a");
  assert.match(texts[0]!, /^Claude is building this/);
  assert.match(texts[0]!, /Sandbox: `bx_\w+` · run `prompt_1` · conversation `convo-1`/);
  assert.deepEqual(texts.slice(1, 3), ["📸 Start", "📸 Finished"]);
  assert.deepEqual(board.files.map((f) => [f.name, f.contentType]), [["01-start.png", "image/png"], ["02-finished.png", "image/png"]]);
  assert.match(texts[3]!, /Added dark mode\.\n\nPull request: https:\/\/github.com\/waifu-devs\/fuwa\/pull\/77\nDrag this card to Done/);
  assert.deepEqual(boat.stopped, ["bx_abcdefg2"]);

  // Dragged to Done: merged, and the sandbox deleted.
  await board.move("a", "done");
  await builder.sweep();
  assert.deepEqual(pulls.merged, [77]);
  assert.match(board.texts("a").at(-1)!, /^🎉 Merged https:\/\/github.com\/waifu-devs\/fuwa\/pull\/77/);
  assert.deepEqual(boat.deleted, ["bx_abcdefg2"]);
  // Nothing happens twice.
  await builder.sweep();
  assert.equal(pulls.merged.length, 1);
  assert.equal(boat.created.length, 1);
});

test("a card labelled site is built in waifu-devs/site", async () => {
  const { board, boat, builder } = setup();
  boat.answer = "Done.\nPull request: https://github.com/waifu-devs/site/pull/5";
  board.view.cards = [card("s", { labelIds: ["ok", "site"] })];
  await builder.sweep();
  await until(() => builder.active === 0);
  assert.match(boat.commands[0]!, /waifu-devs\/site/);
  assert.match(board.texts("s").at(-1)!, /site\/pull\/5/);
});

test("no pull request fails the card, and moving it back to To do tries again", async () => {
  const { board, boat, builder } = setup();
  boat.answer = "The card is unclear: which picker?";
  board.view.cards = [card("a")];
  await builder.sweep();
  await until(() => builder.active === 0 && board.texts("a").some((t) => t.startsWith("⚠️")));
  const failed = board.texts("a").at(-1)!;
  assert.match(failed, /^⚠️ Couldn't finish this: Claude didn't open a pull request/);
  assert.match(failed, /which picker\?/);
  assert.equal(boat.stopped.length, 1);

  // Still in Doing: nothing new.
  await builder.sweep();
  assert.equal(boat.created.length, 1);
  // Back to To do: a new try.
  boat.answer = `Ok.\nPull request: ${PR}`;
  await board.move("a", "todo");
  await builder.sweep();
  await until(() => builder.active === 0 && board.texts("a").at(-1)!.startsWith("✅"));
  assert.equal(boat.created.length, 2);
});

test("a sandbox that can't check out the repository fails the card", async () => {
  const { board, boat, builder } = setup();
  boat.setupExit = 1;
  board.view.cards = [card("a")];
  await builder.sweep();
  await until(() => builder.active === 0);
  assert.match(board.texts("a").at(-1)!, /couldn't check out waifu-devs\/fuwa/);
  assert.equal(boat.prompts.length, 0);
});

test("no more than maxSandboxes cards are built at once", async () => {
  const { board, boat, builder } = setup({ maxSandboxes: 1 });
  boat.runningFor = 5;
  board.view.cards = [card("a"), card("b")];
  await builder.sweep();
  assert.equal(builder.active, 1);
  await until(() => boat.created.length === 2);
  await until(() => builder.active === 0);
});

test("a restart picks up a card from its thread", async () => {
  const { board, boat, builder } = setup();
  board.view.cards = [card("a", { columnId: "doing" })];
  board.threads.set("a", [
    { id: "p1", at: 5, files: [], content: startPost("waifu-devs/fuwa", "roadmap/x", { id: "bx_qqqqqqqq", promptId: "prompt_1", conversationId: "convo-1" }) },
    { id: "p2", at: 6, files: ["01-start.png"], content: "📸 Start" },
  ]);
  await builder.sweep();
  await until(() => builder.active === 0);
  assert.equal(boat.created.length, 0);
  // The screenshot already posted isn't posted again.
  assert.deepEqual(board.files.map((f) => f.name), ["02-finished.png"]);
  assert.match(board.texts("a").at(-1)!, /^✅/);
});

test("a refused merge is said once, and tried again when the card moves", async () => {
  const { board, pulls, builder } = setup();
  board.view.cards = [card("a", { columnId: "done", updatedAt: 10 })];
  board.threads.set("a", [
    { id: "p1", at: 1, files: [], content: startPost("waifu-devs/fuwa", "roadmap/x", { id: "bx_qqqqqqqq", promptId: "prompt_1", conversationId: "c" }) },
    { id: "p2", at: 2, files: [], content: `✅ Done!\n\nPull request: ${PR}` },
  ]);
  pulls.refuse = "Pull Request is not mergeable";
  await builder.sweep();
  await builder.sweep();
  const said = board.texts("a").filter((t) => t.startsWith("Couldn't merge"));
  assert.equal(said.length, 1);
  assert.match(said[0]!, /not mergeable/);
  pulls.refuse = "";
  await board.move("a", "doing");
  await board.move("a", "done");
  await builder.sweep();
  assert.deepEqual(pulls.merged, [77]);
});

test("a pull request merged by hand still closes the card out", async () => {
  const { board, pulls, boat, builder } = setup();
  pulls.merged.push(77);
  board.view.cards = [card("a", { columnId: "done" })];
  board.threads.set("a", [
    { id: "p1", at: 1, files: [], content: startPost("waifu-devs/fuwa", "roadmap/x", { id: "bx_qqqqqqqq", promptId: "prompt_1", conversationId: "c" }) },
    { id: "p2", at: 2, files: [], content: `Pull request: ${PR}` },
  ]);
  await builder.sweep();
  assert.match(board.texts("a").at(-1)!, /^🎉 Merged/);
  assert.deepEqual(boat.deleted, ["bx_qqqqqqqq"]);
});

test("the last response skips partial and other runs' text", () => {
  assert.equal(
    lastResponse(
      [
        { type: "response", taskId: "p2", data: { content: "other run" } },
        { type: "response", taskId: "p1", data: { content: "typing", is_streaming: true } },
        { type: "prompt", taskId: "p1", data: {} },
        { type: "response", taskId: "p1", data: { content: "final" } },
      ],
      "p1",
    ),
    "final",
  );
  assert.equal(lastResponse([], "p1"), "");
});
