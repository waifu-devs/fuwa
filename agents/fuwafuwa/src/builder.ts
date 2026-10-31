// The roadmap builder: cards in a board's To do column that carry the
// Approved label are built by Claude Code in a boat.dev sandbox each. The
// card moves to Doing, its thread gets the screenshots Claude saves as it
// works and then a summary with the pull request, and dragging the card to
// a done column merges that pull request and deletes the sandbox.
//
// Who may approve a card, and who may merge it, is whoever may change labels
// and move cards on the board: Manage Cards. The agent's posts in each
// card's thread are all the state it keeps (roadmap.ts reads them back).
import { MediaPurpose, type Agent } from "@waifu-devs/fuwa";
import { BoatError, type Boat, type BoatEvent, type PromptRun } from "./boat.ts";
import { MergeRefused, type MergeMethod, type Pulls } from "./github.ts";
import {
  SHOTS_DIR,
  branchFor,
  contentTypeOf,
  doingColumn,
  failedPost,
  findByName,
  mergedPost,
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
  type CardState,
  type Column,
  type Named,
  type Post,
} from "./roadmap.ts";

export interface CardInfo {
  id: string;
  title: string;
  columnId: string;
  labelIds: string[];
  /** Milliseconds since 1970. */
  updatedAt: number;
}

export interface BoardView {
  columns: Column[];
  labels: Named[];
  cards: CardInfo[];
  /** Each card's body, by its id. */
  bodies: Map<string, string>;
}

export interface ShotFile {
  name: string;
  bytes: Uint8Array;
  contentType: string;
}

/** The board, as the builder uses it. */
export interface BoardPort {
  read(): Promise<BoardView>;
  move(cardId: string, columnId: string): Promise<void>;
  /** The agent's own posts in a card's thread. */
  posts(cardId: string): Promise<Post[]>;
  /** Posts in a card's thread; the post's id. */
  post(cardId: string, content: string, file?: ShotFile): Promise<string>;
  edit(postId: string, content: string): Promise<void>;
}

export type BoatPort = Pick<
  Boat,
  "create" | "waitReady" | "run" | "prompt" | "promptRun" | "events" | "interrupt" | "download" | "stop" | "delete"
>;
export type PullsPort = Pick<Pulls, "get" | "merge">;

export interface BuilderOptions {
  board: BoardPort;
  boat: BoatPort;
  /** Without it, cards are built but nothing is merged. */
  pulls?: PullsPort;
  /** Where cards are built, "owner/name"; the first unless a card's label names another. */
  repos: string[];
  /** The label that says a card may be built ("Approved"). */
  label: string;
  /** The column names to pick from and to move to while building; found by usual names when empty. */
  todo?: string;
  doing?: string;
  /** How many cards are built at once. */
  maxSandboxes: number;
  sandbox: { type?: string; environment?: string; from?: string; ttlSeconds: number };
  model?: string;
  reasoningEffort?: string;
  /** How often a sandbox is looked at while Claude works. */
  pollMs: number;
  /** How long Claude may take on a card before it's stopped. */
  maxMs: number;
  mergeMethod: MergeMethod;
  log: (line: string) => void;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

/** At most this many screenshots go to one card. */
const MAX_SHOTS = 40;
/** And none bigger than this. */
const MAX_SHOT_BYTES = 20 * 1024 * 1024;

export class Builder {
  #o: BuilderOptions;
  #now: () => number;
  #sleep: (ms: number) => Promise<void>;
  #states = new Map<string, CardState>();
  #active = new Map<string, AbortController>();
  /** The card's updatedAt when a merge was last tried, so a refused merge isn't tried again until it moves. */
  #mergeTried = new Map<string, number>();
  #sweeping: Promise<void> | undefined;
  #again = false;
  #warned = new Set<string>();

  constructor(options: BuilderOptions) {
    this.#o = options;
    this.#now = options.now ?? Date.now;
    this.#sleep = options.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
  }

  /** Cards being built now. */
  get active(): number {
    return this.#active.size;
  }

  /** Looks at the board: starts cards, picks up ones a restart left, merges ones in Done. Calls during one run it once more after. */
  sweep(): Promise<void> {
    if (this.#sweeping) {
      this.#again = true;
      return this.#sweeping;
    }
    this.#sweeping = (async () => {
      try {
        do {
          this.#again = false;
          await this.#sweepOnce();
        } while (this.#again);
      } finally {
        this.#sweeping = undefined;
      }
    })();
    return this.#sweeping;
  }

  /** A card was deleted: stop building it. */
  forget(cardId: string): void {
    const st = this.#states.get(cardId);
    this.#active.get(cardId)?.abort();
    this.#states.delete(cardId);
    if (st?.sandboxId) void this.#o.boat.delete(st.sandboxId).catch(() => this.#o.log("Deleting a sandbox failed."));
  }

  /** Stops following sandboxes (they keep running; a restart picks them up from the card's thread). */
  stop(): void {
    for (const c of this.#active.values()) c.abort();
  }

  async #sweepOnce(): Promise<void> {
    const o = this.#o;
    const board = await o.board.read();
    const todo = todoColumn(board.columns, o.todo);
    const label = findByName(board.labels, o.label);
    if (!todo || !label) {
      this.#warnOnce(!todo ? "todo" : "label", !todo ? "The roadmap board has no To do column." : `The roadmap board has no ${o.label} label.`);
      return;
    }
    const done = new Set(board.columns.filter((c) => c.done).map((c) => c.id));
    const names = new Map(board.labels.map((l) => [l.id, l.name]));
    for (const card of board.cards) {
      if (!card.labelIds.includes(label.id)) continue;
      const st = await this.#state(card.id);
      if (done.has(card.columnId)) {
        if (st.phase === "review" && !this.#active.has(card.id)) await this.#merge(card, st);
        continue;
      }
      if (this.#active.has(card.id)) continue;
      if (st.phase === "working") {
        this.#run(card, st, () => this.#resume(card, st));
        continue;
      }
      if (card.columnId !== todo.id || !shouldStart(st, card.updatedAt)) continue;
      if (this.#active.size >= o.maxSandboxes) {
        this.#warnOnce("full", `${this.#active.size} card(s) are being built; the rest wait their turn.`);
        continue;
      }
      this.#warned.delete("full");
      const repo = repoFor(
        card.labelIds.map((id) => names.get(id) ?? ""),
        o.repos,
      );
      const fresh = stateOf([]);
      this.#states.set(card.id, fresh);
      fresh.phase = "working";
      fresh.repo = repo;
      fresh.at = this.#now();
      const body = board.bodies.get(card.id) ?? "";
      this.#run(card, fresh, () => this.#start(card, fresh, body, board.columns));
    }
  }

  async #state(cardId: string): Promise<CardState> {
    let st = this.#states.get(cardId);
    if (!st) {
      st = stateOf(await this.#o.board.posts(cardId));
      this.#states.set(cardId, st);
    }
    return st;
  }

  /** Runs a card's work in the background; anything that goes wrong fails the card. */
  #run(card: CardInfo, st: CardState, work: (signal: AbortSignal) => Promise<void>): void {
    const ctl = new AbortController();
    this.#active.set(card.id, ctl);
    void (async () => {
      try {
        await work(ctl.signal);
      } catch (err) {
        if (ctl.signal.aborted) return;
        this.#o.log(`Building a card failed${err instanceof BoatError ? ` (${err.message})` : ""}.`);
        await this.#fail(card, st, reasonOf(err)).catch(() => this.#o.log("Saying a card failed failed too."));
      } finally {
        if (this.#active.get(card.id) === ctl) this.#active.delete(card.id);
      }
      // A slot is free, and the card may already be in Done.
      void this.sweep().catch(() => this.#o.log("Looking at the roadmap failed."));
    })();
  }

  async #start(card: CardInfo, st: CardState, body: string, columns: Column[]): Promise<void> {
    const o = this.#o;
    const branch = branchFor(card);
    const dir = `/home/user/${st.repo.split("/")[1]}`;
    st.startPost = await o.board.post(card.id, startPost(st.repo, branch));
    const doing = doingColumn(columns, o.doing);
    if (doing && doing.id !== card.columnId) {
      await o.board.move(card.id, doing.id).catch(() => o.log("Moving a card to Doing failed."));
    }
    const box = await o.boat.create({
      ...o.sandbox,
      name: `roadmap: ${card.title}`,
      labels: { "fuwa-card": card.id },
      idempotencyKey: `fuwa-card-${card.id}-${st.startPost}`,
    });
    st.sandboxId = box.id;
    await o.board.edit(st.startPost, startPost(st.repo, branch, { id: box.id }));
    await o.boat.waitReady(box.id);
    const setup = await o.boat.run(box.id, setupCommand(st.repo, dir), 300);
    if (setup.exitCode !== 0) throw new Failure(`the sandbox couldn't check out ${st.repo} (is GitHub access on in its boat environment?)`);
    const run = await o.boat.prompt(box.id, taskPrompt({ title: card.title, body, repo: st.repo, dir, branch }), {
      model: o.model,
      reasoningEffort: o.reasoningEffort,
    });
    st.promptId = run.promptId;
    st.conversationId = run.conversationId;
    await o.board.edit(st.startPost, startPost(st.repo, branch, { id: box.id, ...run }));
    o.log("Started building a roadmap card.");
    await this.#follow(card, st, this.#now());
  }

  /** A card a restart left half way. */
  async #resume(card: CardInfo, st: CardState): Promise<void> {
    if (!st.sandboxId || !st.promptId) throw new Failure("the agent restarted before Claude got the task");
    this.#o.log("Picking up a roadmap card a restart left.");
    await this.#follow(card, st, st.at);
  }

  async #follow(card: CardInfo, st: CardState, since: number, signal = this.#active.get(card.id)?.signal): Promise<void> {
    const o = this.#o;
    let errors = 0;
    for (;;) {
      await this.#sleep(o.pollMs);
      if (signal?.aborted) return;
      let run: PromptRun;
      try {
        await this.#postShots(card, st);
        run = await o.boat.promptRun(st.sandboxId, st.promptId);
        errors = 0;
      } catch (err) {
        // boat or the instance having a bad minute shouldn't end a run.
        if (++errors >= 10) throw err;
        continue;
      }
      if (run.done) return this.#finish(card, st, run);
      if (this.#now() - since > o.maxMs) {
        await o.boat.interrupt(st.sandboxId, st.conversationId).catch(() => {});
        throw new Failure(`Claude took longer than ${Math.round(o.maxMs / 3_600_000)} hours`);
      }
    }
  }

  async #postShots(card: CardInfo, st: CardState): Promise<void> {
    const o = this.#o;
    if (st.posted.size >= MAX_SHOTS) return;
    const ls = await o.boat.run(st.sandboxId, `ls -1 '${SHOTS_DIR}' 2>/dev/null || true`, 30);
    for (const name of shotsIn(ls.stdout)) {
      if (st.posted.has(name) || st.posted.size >= MAX_SHOTS) continue;
      const bytes = await o.boat.download(st.sandboxId, `${SHOTS_DIR}/${name}`);
      if (bytes.byteLength > 0 && bytes.byteLength <= MAX_SHOT_BYTES) {
        await o.board.post(card.id, shotPost(name), { name, bytes, contentType: contentTypeOf(name) });
      }
      st.posted.add(name);
    }
  }

  async #finish(card: CardInfo, st: CardState, run: PromptRun): Promise<void> {
    const o = this.#o;
    await this.#postShots(card, st).catch(() => o.log("Posting the last screenshots failed."));
    const text = lastResponse(await o.boat.events(st.sandboxId, st.conversationId, 100), st.promptId);
    const summary = summaryOf(text);
    const pr = prIn(text, st.repo);
    if (run.status !== "finished") throw new Failure(`Claude's run ended (${run.status})`, summary);
    if (!pr) throw new Failure("Claude didn't open a pull request", summary);
    await o.board.post(card.id, reviewPost(summary, pr));
    Object.assign(st, { phase: "review", pr, at: this.#now() });
    o.log("A roadmap card is ready for review.");
    // Its disk stays 14 days, for a look; merging deletes it.
    await o.boat.stop(st.sandboxId).catch(() => o.log("Stopping a sandbox failed."));
  }

  async #merge(card: CardInfo, st: CardState): Promise<void> {
    const o = this.#o;
    if (!o.pulls) {
      this.#warnOnce("pulls", "A card is in Done, but without a GitHub token nothing is merged.");
      return;
    }
    if (this.#mergeTried.get(card.id) === card.updatedAt) return;
    this.#mergeTried.set(card.id, card.updatedAt);
    const number = prNumber(st.pr);
    try {
      const pr = await o.pulls.get(st.repo, number);
      if (!pr.merged && pr.state === "closed") {
        await o.board.post(card.id, `The pull request ${st.pr} was closed without merging, so there's nothing to merge.`);
        return;
      }
      if (!pr.merged) await o.pulls.merge(st.repo, number, o.mergeMethod);
    } catch (err) {
      if (!(err instanceof MergeRefused)) {
        o.log("Merging a pull request failed.");
        this.#mergeTried.delete(card.id);
        return;
      }
      await o.board.post(
        card.id,
        `Couldn't merge ${st.pr}: GitHub said "${err.message}". Move the card out of Done and back once that's sorted to try again.`,
      );
      return;
    }
    await o.board.post(card.id, mergedPost(st.pr));
    Object.assign(st, { phase: "merged", at: this.#now() });
    o.log("Merged a roadmap card's pull request.");
    if (st.sandboxId) await o.boat.delete(st.sandboxId).catch(() => o.log("Deleting a sandbox failed."));
  }

  async #fail(card: CardInfo, st: CardState, reason: string, summary = ""): Promise<void> {
    await this.#o.board.post(card.id, failedPost(reason, summary));
    Object.assign(st, { phase: "failed", at: this.#now() });
    if (st.sandboxId) await this.#o.boat.stop(st.sandboxId).catch(() => {});
  }

  #warnOnce(key: string, line: string) {
    if (this.#warned.has(key)) return;
    this.#warned.add(key);
    this.#o.log(line);
  }
}

/** A card that can't be finished, and why, in words for the card's thread. */
export class Failure extends Error {
  readonly summary: string;
  constructor(reason: string, summary = "") {
    super(reason);
    this.name = "Failure";
    this.summary = summary;
  }
}

function reasonOf(err: unknown): string {
  if (err instanceof Failure) return err.message + (err.summary ? `\n\n${err.summary}` : "");
  if (err instanceof BoatError) {
    if (err.code === "provider_not_configured") return "Claude isn't signed in on boat.dev (Agents & Proxying)";
    if (err.status === 429) return "boat.dev's limit on sandboxes is full right now";
    if (err.status === 402) return "boat.dev wants a payment";
    return `boat.dev had a problem (${err.status}${err.code ? `, ${err.code}` : ""})`;
  }
  return "something went wrong on the agent's side";
}

/** What Claude said last in the run: the newest whole response with text. */
export function lastResponse(events: readonly BoatEvent[], promptId: string): string {
  for (const e of events) {
    if (e.type !== "response" || (e.taskId && e.taskId !== promptId)) continue;
    const d = e.data ?? {};
    if (d.is_streaming || d.is_reverted || typeof d.content !== "string" || !d.content.trim()) continue;
    return d.content;
  }
  return "";
}

/** The roadmap board on fuwa, through the agent. */
export function fuwaBoard(agent: Agent, serverId: string, channelId: string): BoardPort {
  const api = agent.api;
  return {
    async read() {
      const res = await api.boards.getBoard({ serverId, channelId });
      return {
        columns: res.columns.map((c) => ({ id: c.id, name: c.name, done: c.done })),
        labels: res.labels.map((l) => ({ id: l.id, name: l.name })),
        cards: res.cards
          .filter((c) => !c.archivedAt)
          .map((c) => ({
            id: c.id,
            title: c.title,
            columnId: c.columnId,
            labelIds: [...c.labelIds],
            updatedAt: c.updatedAt ? Number(c.updatedAt.seconds) * 1000 + Math.floor(c.updatedAt.nanos / 1e6) : 0,
          })),
        bodies: new Map(res.bodies.map((m) => [m.id, m.content])),
      };
    },
    async move(cardId, columnId) {
      await api.boards.moveCard({ serverId, channelId, cardId, columnId, beforeId: "" });
    },
    async posts(cardId) {
      const out: Post[] = [];
      let beforeId = "";
      for (let page = 0; page < 20; page++) {
        const res = await api.messages.listMessages({ serverId, channelId, threadId: cardId, limit: 100, beforeId });
        for (const m of res.messages) {
          if (m.authorId !== agent.me.id) continue;
          const at = m.createdAt ? Number(m.createdAt.seconds) * 1000 + Math.floor(m.createdAt.nanos / 1e6) : 0;
          out.push({ id: m.id, content: m.content, at, files: m.attachments.map((a) => a.filename) });
        }
        if (res.messages.length < 100) break;
        beforeId = res.messages[0]!.id;
      }
      return out;
    },
    async post(cardId, content, file) {
      const attachments = [];
      if (file) {
        const media = await agent.upload({ purpose: MediaPurpose.ATTACHMENT, data: file.bytes, contentType: file.contentType, serverId });
        attachments.push({ url: media.url, filename: file.name, contentType: file.contentType });
      }
      const message = await agent.send(serverId, channelId, { content, threadId: cardId, attachments });
      return message.id;
    },
    async edit(postId, content) {
      await api.messages.updateMessage({ serverId, channelId, messageId: postId, content });
    },
  };
}
