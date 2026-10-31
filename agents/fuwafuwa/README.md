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

## Roadmap builder

With `ROADMAP_CHANNEL` and `BOAT_API_KEY` set, it also builds the roadmap. A
card in the board's To do column that carries the `Approved` label gets a
[boat.dev](https://boat.dev) sandbox of its own, and the Claude Code that's
signed in there builds it:

1. The card moves to Doing, and its thread says it's been picked up, with the
   repository, the branch (`roadmap/<title>-<id>`) and the sandbox.
2. The sandbox checks the repository out (`gh repo clone`, with the GitHub
   access its boat environment gives it), and Claude gets the card's title and
   body with what to do: build it on that branch, run the checks, save
   screenshots in `~/roadmap-shots/` as it goes, push and open a pull request.
3. Every 20 seconds the agent posts the screenshots Claude saved since in the
   card's thread ("📸 Empty state" for `03-empty-state.png`).
4. When Claude is done the thread gets its summary and the pull request, and
   the sandbox is stopped (boat keeps its disk for 14 days). No pull request, a
   run that failed or one longer than `ROADMAP_HOURS` is said in the thread
   instead; moving the card back to To do tries again.
5. Dragging the card to a done column merges the pull request (squash by
   default) and deletes the sandbox. If GitHub won't merge it (conflicts,
   checks), the thread says why; move the card out of Done and back to try
   again.

A card goes to the first of `ROADMAP_REPOS` unless one of its labels is named
like another one (a `site` label for `waifu-devs/site`). At most
`ROADMAP_SANDBOXES` cards are built at once; the rest wait their turn.

Approving and merging are whoever may change labels and move cards: Manage
Cards. Deny it to @everyone on the roadmap board and allow it to a team role,
since an approved card's words become Claude's instructions, with push
access.

Like triage, it keeps no state of its own: its posts in each card's thread
are read back after a restart, so a card that was being built is picked up
again, and screenshots already posted aren't posted twice.

On boat.dev, once: connect Claude (Agents & Proxying), connect GitHub, and
let the environment hand both to sandboxes
(`boat env set base --github true --agents-credentials true`). A saved
snapshot with the repositories' tools installed (`BOAT_SNAPSHOT`) makes
each card start faster.

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
| `ROADMAP_CHANNEL` | `<server id>/<channel id>` of the roadmap board, in a server the agent is in with Manage Cards there. Without it, no roadmap builder |
| `ROADMAP_LABEL` | `Approved` by default: the label that says a card may be built |
| `ROADMAP_TODO`, `ROADMAP_DOING` | the columns to pick cards from and move them to; found by their usual names (To do, Doing, In progress...) when not set |
| `ROADMAP_REPOS` | `waifu-devs/fuwa` by default: where cards are built, comma separated |
| `ROADMAP_SANDBOXES` | how many cards are built at once, `2` by default |
| `ROADMAP_HOURS` | how long Claude may take on one card, `4` by default |
| `ROADMAP_MERGE` | `squash` (default), `merge` or `rebase` |
| `ROADMAP_GITHUB_TOKEN` | a fine-grained token with Contents and Pull requests: read and write on `ROADMAP_REPOS`, to merge. Without it, nothing is merged |
| `BOAT_API_KEY` | a boat.dev API key (`boat_...`). A secret. Without it, no roadmap builder |
| `BOAT_ENVIRONMENT` | the boat environment sandboxes start with; boat's `base` by default |
| `BOAT_SNAPSHOT` | a saved snapshot to start sandboxes from |
| `BOAT_TYPE` | `small`, `default` (default), `large` or `xlarge` |
| `BOAT_MODEL`, `BOAT_EFFORT` | Claude Code's model and reasoning effort; the ones set on boat.dev by default |

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
`FUWAFUWA_GITHUB_TOKEN` and `FUWAFUWA_ANTHROPIC_API_KEY`, set by hand. To turn
the roadmap builder on, add the shared variables `FUWAFUWA_BOAT_API_KEY` and
`FUWAFUWA_ROADMAP_GITHUB_TOKEN`, then give the service `ROADMAP_CHANNEL`,
`ROADMAP_REPOS` (`waifu-devs/fuwa,waifu-devs/site`), `BOAT_API_KEY` and
`ROADMAP_GITHUB_TOKEN` from them in `.railway/railway.ts`.

Railway's GitHub app needs access to `waifu-devs/fuwa`. To set it up: make an agent named `fuwafuwa` on fuwa.chat, add it to the
servers it should listen in and to the one with the team's channel, set the
three shared variables, then put the channel in `FEEDBACK_CHANNEL`.
