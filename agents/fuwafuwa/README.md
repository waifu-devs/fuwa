# @fuwafuwa

The agent on fuwa.chat that takes feedback about fuwa. It's built on the
TypeScript SDK ([docs/sdk.md](../../docs/sdk.md)), from npm like any other
agent, and runs on its own: it reaches the instance at its public address and
keeps nothing.

Mention it with what you think:

> @fuwafuwa the emoji picker is slow on my phone

It thanks you and posts it in the team's channel (`FEEDBACK_CHANNEL`), as
"Feedback from @you" and your words quoted (mentions and ids made harmless). A
mention with nothing else ("@fuwafuwa", "help") gets a short explanation.

## Triage

Every `TRIAGE_HOURS` (on the hour from midnight UTC: 00:00, 06:00, ...) it
looks through its posts in that channel from the last `TRIAGE_DAYS` that
haven't been handled, and sends their words (no names) to Claude with the open
GitHub issues labelled `feedback`. Claude groups what's about the same thing
and says, per group, whether it's:

- a **new issue**: filed with Claude's title and summary, then every piece of
  feedback quoted, and the `feedback` label;
- already an **open issue**: the feedback is added to it as a comment;
- **nothing to file** (thanks, questions, noise), with a short reason.

The plan is checked before anything is filed (`checkPlan`: each post in one
group at most, only issues it was shown, a title for every new one). Then each
post is edited to say what happened ("→ Filed as #12: …", "→ Added to #9: …",
"→ Not filed: praise"). That mark is the only state: a post without one is
picked up next time, so anything that failed, or that Claude left out, waits
for the next run. Feedback is untrusted text, and the prompt tells Claude so;
the worst it can do is word an issue.

Claude is `claude-opus-5-5` at medium effort with structured output, and
`fallbacks: "default"`, so a request a safety classifier declines is run again
on the model Anthropic recommends for it instead of failing.

## Settings

| Variable | |
| --- | --- |
| `FUWA_URL` | the instance, `https://fuwa.chat` by default |
| `FUWA_TOKEN` | the agent's token (Settings > Agents). A secret |
| `FEEDBACK_CHANNEL` | `<server id>/<channel id>`, in a server the agent is in. Required |
| `TRIAGE_HOURS` | how often triage runs, `6` by default; `0` never |
| `TRIAGE_DAYS` | how far back it looks, `14` by default |
| `TRIAGE_NOW` | `1` also runs it once right after starting |
| `ANTHROPIC_API_KEY` | for Claude. Without it, no triage |
| `ANTHROPIC_MODEL` | `claude-opus-5-5` by default |
| `GITHUB_TOKEN` | a fine-grained token with Issues: read and write on `GITHUB_REPO`, nothing else. Without it, no triage |
| `GITHUB_REPO` | `waifu-devs/fuwa` by default |
| `GITHUB_LABEL` | `feedback` by default: on every issue it files, and how it finds open ones. Make sure the repository has it |

## Running it

```sh
pnpm install
FUWA_TOKEN=... FEEDBACK_CHANNEL=... pnpm start   # Node 24 runs the TypeScript as it is
pnpm typecheck && pnpm test
```

## Where it runs

`fuwafuwa` is a service in fuwa.chat's Railway project (`.railway/railway.ts`),
one replica, declared once `FEEDBACK_CHANNEL` is set there. Railway builds it
from this folder's Dockerfile on master whenever a commit changes it, after
the commit's checks pass; fuwa's own image and deploys leave it alone. Its secrets are the project's shared variables `FUWAFUWA_TOKEN`,
`FUWAFUWA_GITHUB_TOKEN` and `FUWAFUWA_ANTHROPIC_API_KEY`, set by hand.

Railway's GitHub app needs access to `waifu-devs/fuwa`. To set it up: make an agent named `fuwafuwa` on fuwa.chat, add it to the
servers it should listen in and to the one with the team's channel, set the
three shared variables, then put the channel in `FEEDBACK_CHANNEL`.
