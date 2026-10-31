// fuwafuwa, the agent that takes feedback about fuwa. Mention it with what you
// think ("@fuwafuwa the emoji picker is slow on my phone") and it thanks you
// and posts it in the team's channel. Every few hours, triage (triage.ts) has
// Claude group what's come in, files an issue per group worth filing (or adds
// to one already open), and marks each post with what it did.
//
//   FUWA_TOKEN=... FEEDBACK_CHANNEL=<server>/<channel> node src/main.ts
//
// FUWA_URL           the instance (https://fuwa.chat)
// FUWA_TOKEN         the agent's token (Settings > Agents), a secret
// FEEDBACK_CHANNEL   "<server id>/<channel id>": where feedback is posted, in a server
//                    the agent is in
// TRIAGE_HOURS       how often triage runs, on the hour from midnight UTC (6; 0 never)
// TRIAGE_DAYS        how far back it looks for feedback it hasn't handled (14)
// TRIAGE_NOW         1: also run once right after starting
// FEEDBACK_PER_HOUR  how much feedback one account may send in an hour (5; unlimited for no limit)
// ANTHROPIC_API_KEY  for Claude, which groups the feedback; without it, no triage
// ANTHROPIC_MODEL    claude-opus-5-5
// GITHUB_TOKEN       a fine-grained token with Issues: read and write on GITHUB_REPO;
//                    without it, no triage
// GITHUB_REPO        where issues go (waifu-devs/fuwa)
// GITHUB_LABEL       on every issue triage files, and how it finds open ones (feedback)
//
// The roadmap builder (builder.ts), on when ROADMAP_CHANNEL and BOAT_API_KEY are set:
// ROADMAP_CHANNEL    "<server id>/<channel id>" of the roadmap board
// ROADMAP_LABEL      the label that says a card may be built (Approved)
// ROADMAP_TODO       the column cards are picked from (To do, Todo or Backlog)
// ROADMAP_DOING      the column a card moves to while it's built (Doing, In progress...)
// ROADMAP_REPOS      where cards are built, owner/name, comma separated (waifu-devs/fuwa);
//                    a card with a label named like another one's goes there
// ROADMAP_SANDBOXES  how many cards are built at once (2)
// ROADMAP_HOURS      how long Claude may take on one card (4)
// ROADMAP_MERGE      squash, merge or rebase (squash)
// ROADMAP_GITHUB_TOKEN  a fine-grained token with Contents and Pull requests: read and write
//                    on ROADMAP_REPOS, to merge; without it nothing is merged
// BOAT_API_KEY       a boat.dev API key, a secret
// BOAT_ENVIRONMENT   the boat environment sandboxes start with (boat's base)
// BOAT_SNAPSHOT      a saved snapshot to start them from instead
// BOAT_TYPE          small, default, large or xlarge (default)
// BOAT_MODEL, BOAT_EFFORT  Claude Code's model and reasoning effort (boat's defaults)
import {
  Agent,
  NotFoundError,
  RateLimitedError,
  UnauthenticatedError,
  messages,
  type Message,
  type MessageContext,
} from "@waifu-devs/fuwa";
import {
  FeedbackPace,
  asksForHelp,
  feedbackOf,
  feedbackOfPost,
  handled,
  nextRun,
  perHourOf,
  teamPost,
  withOutcome,
} from "./feedback.ts";
import { Boat } from "./boat.ts";
import { Builder, fuwaBoard } from "./builder.ts";
import { GitHub, Pulls, type MergeMethod } from "./github.ts";
import { parseRepos } from "./roadmap.ts";
import { ClaudeGrouper, triage, type Pending } from "./triage.ts";

const env = process.env;
const url = env.FUWA_URL || "https://fuwa.chat";
const token = env.FUWA_TOKEN;
if (!token) {
  console.error("Set FUWA_TOKEN to the agent's token.");
  process.exit(1);
}
const team = parseChannel(env.FEEDBACK_CHANNEL, "FEEDBACK_CHANNEL", "where feedback is posted");
const instance = new URL(url).host;

const github = env.GITHUB_TOKEN
  ? new GitHub({
      repo: env.GITHUB_REPO || "waifu-devs/fuwa",
      token: env.GITHUB_TOKEN,
      label: env.GITHUB_LABEL || "feedback",
    })
  : undefined;
const grouper = env.ANTHROPIC_API_KEY
  ? new ClaudeGrouper({ apiKey: env.ANTHROPIC_API_KEY, model: env.ANTHROPIC_MODEL || "claude-opus-5-5" })
  : undefined;
const hours = Number(env.TRIAGE_HOURS || 6);
const lookbackMs = Number(env.TRIAGE_DAYS || 14) * 24 * 60 * 60_000;
const perHour = perHourOf(env.FEEDBACK_PER_HOUR);
if (perHour === undefined) {
  console.error("Set FEEDBACK_PER_HOUR to a whole number from 1 up, or unlimited.");
  process.exit(1);
}
const pace = new FeedbackPace(perHour);

const HELP =
  "Hi! I take feedback about fuwa. Mention me with what you think, what broke or what you wish it did, " +
  'like "@fuwafuwa the emoji picker is slow on my phone", and I\'ll pass it on to the team.';

const agent = new Agent({
  url,
  token,
  onError: () => console.error("A handler failed."),
});

const roadmap = env.ROADMAP_CHANNEL
  ? parseChannel(env.ROADMAP_CHANNEL, "ROADMAP_CHANNEL", "the roadmap board")
  : undefined;
const builder = roadmap && env.BOAT_API_KEY ? makeBuilder(roadmap) : undefined;
if (roadmap && !builder) console.log("ROADMAP_CHANNEL is set, but nothing is built without BOAT_API_KEY.");

function makeBuilder(board: { serverId: string; channelId: string }): Builder {
  const merge = (env.ROADMAP_MERGE || "squash") as MergeMethod;
  const max = Number(env.ROADMAP_SANDBOXES || 2);
  const hours = Number(env.ROADMAP_HOURS || 4);
  let repos: string[] = [];
  try {
    repos = parseRepos(env.ROADMAP_REPOS);
  } catch (err) {
    console.error((err as Error).message);
    process.exit(1);
  }
  if (!["squash", "merge", "rebase"].includes(merge) || !(max >= 1) || !(hours > 0)) {
    console.error("Set ROADMAP_MERGE to squash, merge or rebase, ROADMAP_SANDBOXES to 1 or more and ROADMAP_HOURS above 0.");
    process.exit(1);
  }
  return new Builder({
    board: fuwaBoard(agent, board.serverId, board.channelId),
    boat: new Boat({ apiKey: env.BOAT_API_KEY! }),
    pulls: env.ROADMAP_GITHUB_TOKEN ? new Pulls({ token: env.ROADMAP_GITHUB_TOKEN }) : undefined,
    repos,
    label: env.ROADMAP_LABEL || "Approved",
    todo: env.ROADMAP_TODO,
    doing: env.ROADMAP_DOING,
    maxSandboxes: Math.floor(max),
    sandbox: {
      type: env.BOAT_TYPE || undefined,
      environment: env.BOAT_ENVIRONMENT || undefined,
      from: env.BOAT_SNAPSHOT || undefined,
      ttlSeconds: 2 * 60 * 60,
    },
    model: env.BOAT_MODEL || undefined,
    reasoningEffort: env.BOAT_EFFORT || undefined,
    pollMs: 20_000,
    maxMs: hours * 60 * 60_000,
    mergeMethod: merge,
    log: (line) => console.log(`Roadmap: ${line}`),
  });
}

if (builder && roadmap) {
  // Card events say something moved; the board is read again a moment later.
  let soon: NodeJS.Timeout | undefined;
  const sweepSoon = () => {
    clearTimeout(soon);
    soon = setTimeout(() => void builder.sweep().catch((err) => report("Looking at the roadmap", err)), 2_000);
  };
  agent.on("cardUpdated", (update, event) => {
    if (event.serverId === roadmap.serverId && update.card?.channelId === roadmap.channelId) sweepSoon();
  });
  agent.on("cardDeleted", (deleted, event) => {
    if (event.serverId === roadmap.serverId && deleted.channelId === roadmap.channelId) builder.forget(deleted.cardId);
  });
  agent.on("ready", sweepSoon);
  setInterval(sweepSoon, 5 * 60_000);
}

agent.on("mention", async (ctx: MessageContext) => {
  const feedback = feedbackOf(ctx.content, agent.me);
  if (asksForHelp(feedback)) {
    await ctx.reply(HELP);
    return;
  }
  const author = await ctx.author();
  // Per account, never per address: the instance says who wrote it.
  const turn = pace.take(author?.id ?? ctx.message.authorId, Date.now());
  if (turn !== "ok") {
    if (turn === "over") await ctx.reply("Thanks! You've sent me a lot this hour, so I'll take more in a little while.");
    return;
  }
  try {
    await agent.send(team.serverId, team.channelId, teamPost(author?.username, feedback));
  } catch (err) {
    report("Passing feedback on", err);
    await ctx.reply("Sorry, I couldn't pass that on just now. Could you try again in a little while?");
    return;
  }
  await ctx.reply("Thanks, got it! I've passed it on to the team.");
});

// The posts the last look found, to mark them by id.
const posts = new Map<string, Message>();

/** The agent's posts in the team's channel that triage hasn't handled, oldest first. */
async function pending(): Promise<Pending[]> {
  posts.clear();
  const since = Date.now() - lookbackMs;
  const out: Pending[] = [];
  for await (const { message } of messages(agent.api, { serverId: team.serverId, channelId: team.channelId })) {
    const at = message.createdAt ? Number(message.createdAt.seconds) * 1000 : 0;
    if (at < since) break;
    if (message.authorId !== agent.me.id || handled(message.content)) continue;
    const feedback = feedbackOfPost(message.content);
    if (!feedback) continue;
    posts.set(message.id, message);
    out.unshift({ id: message.id, feedback });
  }
  return out;
}

let running = false;
async function runTriage() {
  if (!github || !grouper || running) return;
  running = true;
  try {
    const result = await triage({
      pending,
      openIssues: () => github.openIssues(),
      grouper,
      createIssue: (title, body) => github.createIssue(title, body),
      comment: (issue, body) => github.comment(issue, body),
      mark: async (p, outcome) => {
        const post = posts.get(p.id);
        if (post) await agent.edit(post, withOutcome(post.content, outcome));
      },
      instance,
      log: (line) => console.log(line),
    });
    console.log(
      `Triage: ${result.filed} filed, ${result.added} added to open issues, ` +
        `${result.skipped} not filed, ${result.waiting} waiting.`,
    );
  } catch (err) {
    report("Triage", err);
  } finally {
    running = false;
  }
}

function schedule() {
  setTimeout(() => void runTriage().finally(schedule), nextRun(Date.now(), hours) - Date.now());
}

function parseChannel(value: string | undefined, name: string, what: string): { serverId: string; channelId: string } {
  const [serverId, channelId, extra] = (value ?? "").split("/");
  if (!serverId || !channelId || extra !== undefined) {
    console.error(`Set ${name} to <server id>/<channel id>, ${what}.`);
    process.exit(1);
  }
  return { serverId, channelId };
}

/** Says what failed, in fixed words: an error's own message can carry what people wrote or where. */
function report(what: string, err: unknown) {
  if (err instanceof RateLimitedError) console.log(`${what} was slowed down by the instance.`);
  else console.error(`${what} failed.`);
}

const triaging = github !== undefined && grouper !== undefined && hours > 0;
agent.on("ready", ({ me, servers }) =>
  console.log(
    `Signed in as @${me.username} in ${servers.length} server(s). ` +
      (triaging
        ? `Triage every ${hours}h into ${github!.repo}.`
        : "No triage (it needs ANTHROPIC_API_KEY, GITHUB_TOKEN and TRIAGE_HOURS above 0).") +
      (builder ? " Building approved roadmap cards." : ""),
  ),
);
agent.on("disconnected", ({ retryInMs }) =>
  console.log(`Lost the connection; trying again in ${Math.round(retryInMs / 1000)}s.`),
);

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, async () => {
    builder?.stop();
    await agent.stop();
    process.exit(0);
  });
}

// The instance may be restarting (every deploy restarts it): wait it out
// rather than exit. A token that doesn't work is the one reason to stop.
for (let wait = 1000; ; wait = Math.min(wait * 2, 60_000)) {
  try {
    await agent.start();
    break;
  } catch (err) {
    if (err instanceof UnauthenticatedError) fail();
    report("Signing in", err);
    console.log(`Trying again in ${wait / 1000}s.`);
    await new Promise((r) => setTimeout(r, wait));
  }
}
// Nothing works without the team's channel.
try {
  await agent.api.channels.getChannel({ serverId: team.serverId, channelId: team.channelId });
} catch (err) {
  if (err instanceof NotFoundError) {
    console.error("FEEDBACK_CHANNEL isn't a channel the agent can see: add it to that server.");
    process.exit(1);
  }
  report("Finding FEEDBACK_CHANNEL", err);
}
if (roadmap && builder) {
  try {
    await agent.api.boards.getBoard(roadmap);
  } catch (err) {
    if (err instanceof NotFoundError) {
      console.error("ROADMAP_CHANNEL isn't a board the agent can see: add it to that server.");
      process.exit(1);
    }
    report("Finding ROADMAP_CHANNEL", err);
  }
}
if (triaging) {
  schedule();
  if (env.TRIAGE_NOW === "1") void runTriage();
}
await agent.closed.catch((err) =>
  err instanceof UnauthenticatedError ? fail() : (report("The connection", err), process.exit(1)),
);

function fail(): never {
  console.error("FUWA_TOKEN doesn't sign in (reset, or the agent was deleted). Set a new one from Settings > Agents.");
  process.exit(1);
}
