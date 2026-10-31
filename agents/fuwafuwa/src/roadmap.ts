// What the roadmap builder decides, kept pure so it can be tested: which
// cards to pick up, which repository and branch each goes to, what Claude is
// asked, and what the agent's own posts in a card's thread say about where
// that card is. Those posts are the only state: the builder reads them again
// after a restart.

/** A board column or label, as BoardService returns them. */
export interface Named {
  id: string;
  name: string;
}

export interface Column extends Named {
  done: boolean;
}

/** One of the agent's posts in a card's thread. */
export interface Post {
  id: string;
  content: string;
  /** Milliseconds since 1970. */
  at: number;
  /** The names of the files on it. */
  files: string[];
}

export type Phase = "none" | "working" | "review" | "merged" | "failed";

/** Where a card is, from the agent's posts in its thread. */
export interface CardState {
  phase: Phase;
  sandboxId: string;
  promptId: string;
  conversationId: string;
  /** The repository its pull request is in ("owner/name"), and its link. */
  repo: string;
  pr: string;
  /** The post that says it's being built, to edit as the sandbox comes up. */
  startPost: string;
  /** When the post that set the phase was made. */
  at: number;
  /** Screenshots already posted, by file name. */
  posted: Set<string>;
}

/** Makes names comparable: "To do", "todo" and "TO-DO" are the same column. */
export function normalize(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]/g, "");
}

export function findByName<T extends Named>(items: readonly T[], ...names: string[]): T | undefined {
  const wanted = names.map(normalize).filter(Boolean);
  return items.find((i) => wanted.includes(normalize(i.name)));
}

/** The column cards are picked from. */
export function todoColumn(columns: readonly Column[], name = ""): Column | undefined {
  return name ? findByName(columns, name) : findByName(columns, "To do", "Todo", "Backlog");
}

/** The column a card moves to while it's built, if the board has one. */
export function doingColumn(columns: readonly Column[], name = ""): Column | undefined {
  return name ? findByName(columns, name) : findByName(columns, "Doing", "In progress", "WIP", "Building");
}

/**
 * The repository a card is built in: the one whose name matches one of its
 * labels ("site" for waifu-devs/site), or the first.
 */
export function repoFor(labelNames: readonly string[], repos: readonly string[]): string {
  const labels = new Set(labelNames.map(normalize));
  return repos.find((r) => labels.has(normalize(r.split("/")[1] ?? ""))) ?? repos[0]!;
}

/** "owner/name" of a repository on GitHub. */
export const REPO = /^[\w.-]+\/[\w.-]+$/;

export function parseRepos(value: string | undefined): string[] {
  const repos = (value || "waifu-devs/fuwa")
    .split(",")
    .map((r) => r.trim())
    .filter(Boolean);
  if (!repos.length || !repos.every((r) => REPO.test(r))) throw new Error("ROADMAP_REPOS must be owner/name, comma separated");
  return repos;
}

/** The branch a card's work goes on: its title in a few words and the end of its id. */
export function branchFor(card: { id: string; title: string }): string {
  const slug = card.title
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40)
    .replace(/-+$/, "");
  return `roadmap/${slug || "card"}-${card.id.slice(-6).toLowerCase()}`;
}

/** Where the screenshots go in the sandbox. */
export const SHOTS_DIR = "/home/user/roadmap-shots";

export interface Task {
  title: string;
  body: string;
  repo: string;
  /** Where the repository is checked out in the sandbox. */
  dir: string;
  branch: string;
}

/** What Claude Code in the sandbox is asked to do. */
export function taskPrompt(t: Task): string {
  const body = t.body.trim() || "(The card has no description: go by its title.)";
  return [
    `You're building a card from the Waifu Devs roadmap in ${t.repo}, which is checked out at ${t.dir}.`,
    "",
    "The card, as people wrote it on the board:",
    "<card>",
    `# ${t.title.trim()}`,
    "",
    body,
    "</card>",
    "",
    "Do this:",
    `1. Start a branch named ${t.branch} from the latest default branch.`,
    "2. Build what the card asks for. Follow the repository's AGENTS.md or CLAUDE.md if it has one, and run its checks for what you changed.",
    `3. Show your progress with screenshots. Whenever there's something to see (the screen you changed, a page, a passing test run), save a PNG in ${SHOTS_DIR}/ named with a two-digit number and a few words, like 01-empty-state.png. Use Chrome on the desktop (the computer tools) or a headless browser. Each one is posted to the card as soon as you save it, so save a few along the way; the last ones should show the finished result.`,
    "4. Commit, push the branch and open a pull request against the default branch with `gh pr create`. Its title says what it does; its body says what changed and how you checked it.",
    "5. End your last message with a short summary for the people who wrote the card, then a last line: `Pull request: <its URL>`.",
    "",
    "If you can't finish it, say why in your last message and don't open a pull request.",
  ].join("\n");
}

/** The shell command that gets a sandbox ready: the repository checked out, the screenshot folder there. */
export function setupCommand(repo: string, dir: string): string {
  const q = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`;
  return [
    `mkdir -p ${q(SHOTS_DIR)}`,
    `if [ -d ${q(dir)}/.git ]; then git -C ${q(dir)} fetch --quiet origin; else gh repo clone ${q(repo)} ${q(dir)} -- --quiet; fi`,
    `git -C ${q(dir)} status --short --branch | head -1`,
  ].join(" && ");
}

/** The screenshots in `ls -1` output, in order. */
export function shotsIn(listing: string): string[] {
  return listing
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => /^[\w.-]{1,120}\.(png|jpe?g|webp)$/i.test(l))
    .sort();
}

export function contentTypeOf(file: string): string {
  const ext = file.toLowerCase().split(".").pop();
  return ext === "png" ? "image/png" : ext === "webp" ? "image/webp" : "image/jpeg";
}

/** "03-empty-state.png" is "Empty state". */
export function captionOf(file: string): string {
  const words = file
    .replace(/\.[a-z]+$/i, "")
    .replace(/^\d+[-_. ]*/, "")
    .replace(/[-_]+/g, " ")
    .trim();
  return words ? words[0]!.toUpperCase() + words.slice(1) : "Screenshot";
}

/** A pull request's link in what Claude said: the last one in the card's repository. */
export function prIn(text: string, repo: string): string {
  const re = new RegExp(`https://github\\.com/${repo.replace(/[.]/g, "\\.")}/pull/(\\d+)`, "gi");
  let last = "";
  for (const m of text.matchAll(re)) last = m[0];
  return last;
}

export function prNumber(url: string): number {
  const m = /\/pull\/(\d+)$/.exec(url);
  return m ? Number(m[1]) : 0;
}

/**
 * What Claude said, made fit for a post: no pull request line (the post has
 * its own), no pings, and not too long.
 */
export function summaryOf(text: string, max = 1500): string {
  let s = text
    .split("\n")
    .filter((l) => !/^\W*pull request:/i.test(l.trim()))
    .join("\n")
    .replace(/@(everyone|here)\b/g, "@\u200b$1")
    .replace(/<@&?\w+>/g, "@someone")
    .trim();
  if (s.length > max) s = `${s.slice(0, max - 1).trimEnd()}…`;
  return s;
}

// The agent's posts, and the lines in them it reads back.

const SANDBOX_LINE = /^Sandbox: `(bx_[23456789a-z]{8})`(?: · run `([\w-]+)` · conversation `([\w-]+)`)?/m;
const REPO_LINE = /^Repository: `([\w.-]+\/[\w.-]+)`, branch `/m;
const PR_LINE = /^Pull request: (https:\/\/github\.com\/([\w.-]+\/[\w.-]+)\/pull\/\d+)/m;

export function startPost(repo: string, branch: string, sandbox?: { id: string; promptId?: string; conversationId?: string }): string {
  const lines = [
    sandbox?.promptId
      ? "Claude is building this in a boat.dev sandbox. Screenshots will show up here as it goes."
      : "Picked this up: starting a boat.dev sandbox for it.",
    `Repository: \`${repo}\`, branch \`${branch}\``,
  ];
  if (sandbox) {
    lines.push(
      sandbox.promptId
        ? `Sandbox: \`${sandbox.id}\` · run \`${sandbox.promptId}\` · conversation \`${sandbox.conversationId ?? ""}\``
        : `Sandbox: \`${sandbox.id}\``,
    );
  }
  return lines.join("\n");
}

export function shotPost(file: string): string {
  return `📸 ${captionOf(file)}`;
}

export function reviewPost(summary: string, pr: string): string {
  return [
    "✅ Done! Here's what Claude said:",
    "",
    summary || "(No summary.)",
    "",
    `Pull request: ${pr}`,
    "Drag this card to Done to merge it.",
  ].join("\n");
}

export const MERGED = "🎉 Merged ";
export const FAILED = "⚠️ Couldn't finish this: ";

export function mergedPost(pr: string): string {
  return `${MERGED}${pr}. The sandbox is deleted.`;
}

export function failedPost(reason: string, summary = ""): string {
  const lines = [`${FAILED}${reason}`];
  if (summary) lines.push("", summary);
  lines.push("", "Move the card back to To do (or edit it there) to try again.");
  return lines.join("\n");
}

/** Reads the agent's posts in a card's thread (in any order) back into where the card is. */
export function stateOf(posts: readonly Post[]): CardState {
  const st: CardState = {
    phase: "none",
    sandboxId: "",
    promptId: "",
    conversationId: "",
    repo: "",
    pr: "",
    startPost: "",
    at: 0,
    posted: new Set(),
  };
  for (const p of [...posts].sort((a, b) => a.at - b.at)) {
    for (const f of p.files) st.posted.add(f);
    const c = p.content;
    const sandbox = SANDBOX_LINE.exec(c);
    const repo = REPO_LINE.exec(c);
    const pr = PR_LINE.exec(c);
    if (repo && c.includes("boat.dev sandbox")) {
      // A new start: whatever came before is over.
      Object.assign(st, { phase: "working", sandboxId: "", promptId: "", conversationId: "", pr: "", repo: repo[1]!, startPost: p.id, at: p.at });
      if (sandbox) {
        st.sandboxId = sandbox[1]!;
        st.promptId = sandbox[2] ?? "";
        st.conversationId = sandbox[3] ?? "";
      }
    } else if (pr && st.phase === "working") {
      Object.assign(st, { phase: "review", pr: pr[1]!, repo: pr[2]!, at: p.at });
    } else if (c.startsWith(MERGED)) {
      Object.assign(st, { phase: "merged", at: p.at });
    } else if (c.startsWith(FAILED)) {
      Object.assign(st, { phase: "failed", at: p.at });
    }
  }
  return st;
}

/**
 * Whether a card in the To do column with the label should be picked up: it
 * never was, or the last try failed and someone has moved it since.
 */
export function shouldStart(st: CardState, cardUpdatedAt: number): boolean {
  if (st.phase === "none") return true;
  return st.phase === "failed" && cardUpdatedAt > st.at;
}
