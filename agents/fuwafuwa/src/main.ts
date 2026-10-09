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
import { GitHub } from "./github.ts";
import { ClaudeGrouper, triage, type Pending } from "./triage.ts";

const env = process.env;
const url = env.FUWA_URL || "https://fuwa.chat";
const token = env.FUWA_TOKEN;
if (!token) {
  console.error("Set FUWA_TOKEN to the agent's token.");
  process.exit(1);
}
const team = parseChannel(env.FEEDBACK_CHANNEL);
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

function parseChannel(value: string | undefined): { serverId: string; channelId: string } {
  const [serverId, channelId, extra] = (value ?? "").split("/");
  if (!serverId || !channelId || extra !== undefined) {
    console.error("Set FEEDBACK_CHANNEL to <server id>/<channel id>, where feedback is posted.");
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
        : "No triage (it needs ANTHROPIC_API_KEY, GITHUB_TOKEN and TRIAGE_HOURS above 0)."),
  ),
);
agent.on("disconnected", ({ retryInMs }) =>
  console.log(`Lost the connection; trying again in ${Math.round(retryInMs / 1000)}s.`),
);

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, async () => {
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
