// What fuwafuwa does with feedback text, kept pure so it's tested on its own.

/** A zero-width space: breaks a mention without changing how the text reads. */
const ZWSP = "\u200b";

/**
 * The feedback in a message that mentions the agent: the text without the
 * agent's own mentions (`<@id>`, `<@!id>` or `@username`), trimmed.
 */
export function feedbackOf(content: string, me: { id: string; username: string }): string {
  const id = escapeRegExp(me.id);
  const name = escapeRegExp(me.username);
  return content
    .replace(new RegExp(`<@!?${id}>[,:]?`, "g"), " ")
    .replace(new RegExp(`(^|[^\\w@])@${name}(?![\\w.]*\\w)[,:]?`, "gi"), "$1 ")
    .replace(/[ \t]{2,}/g, " ")
    .trim();
}

/** Text that isn't feedback: nothing, or asking what the agent does. */
export function asksForHelp(feedback: string): boolean {
  return /^(|help|\?|hi|hello|hey|what (do|can) you do\??)$/i.test(feedback.trim());
}

/**
 * Feedback as it can be quoted elsewhere: nobody's id, no pings. fuwa's
 * mentions and emoji become plain names, and `@word` is broken with a
 * zero-width space so neither fuwa nor GitHub pings anyone.
 */
export function neutral(text: string): string {
  return text
    .replace(/<@!?[0-9A-Za-z]+>/g, "@someone")
    .replace(/<@&[0-9A-Za-z]+>/g, "@a role")
    .replace(/<#[0-9A-Za-z]+>/g, "#a channel")
    .replace(/<a?:([\w~-]+):[0-9A-Za-z]+>/g, ":$1:")
    .replace(/@(?=[\w-])/g, `@${ZWSP}`);
}

/**
 * Links that don't work as links, broken the way security write-ups do it:
 * `hxxps[:]//`, any other `scheme[:]//`, `[/]/host` for addresses without a
 * scheme and `www[.]`. GitHub neither links nor loads them: no picture
 * fetched from someone's server, no link to click in an issue.
 */
export function defanged(text: string): string {
  return text
    .replace(/\b(h)ttp(?=s?:\/\/)/gi, "$1xxp")
    .replace(/:\/\//g, "[:]//")
    .replace(/(^|[\s("'=<])\/\/(?=[\w-])/g, "$1[/]/")
    .replace(/\b(www)\./gi, "$1[.]");
}

/**
 * Text Claude wrote from feedback, as it can go to GitHub: like the feedback
 * itself, it names and pings nobody ({@link neutral}) and carries no live
 * links ({@link defanged}), whatever the feedback asked it to write.
 */
export function forGitHub(text: string): string {
  return defanged(neutral(text));
}

/**
 * Each account's feedback in the last hour, so one person can't fill the
 * team's channel (and each triage) on their own. Kept in memory: a restart
 * starts everyone over.
 */
export class FeedbackPace {
  #perHour: number;
  #sent = new Map<string, number[]>();

  constructor(perHour: number) {
    this.#perHour = perHour;
  }

  /** Counts one piece of feedback from `accountId` at `now`; false when it's one too many. */
  take(accountId: string, now: number): boolean {
    const since = now - 60 * 60_000;
    if (this.#sent.size > 10_000) {
      for (const [id, times] of this.#sent) if (times.every((t) => t <= since)) this.#sent.delete(id);
    }
    const times = (this.#sent.get(accountId) ?? []).filter((t) => t > since);
    if (times.length >= this.#perHour) {
      this.#sent.set(accountId, times);
      return false;
    }
    times.push(now);
    this.#sent.set(accountId, times);
    return true;
  }
}

function quote(text: string): string {
  return text
    .split("\n")
    .map((l) => (l ? `> ${l}` : ">"))
    .join("\n");
}

const HEAD = "Feedback from ";
/** Where the outcome starts on a post triage has handled. */
const OUTCOME = "\n\n→ ";

/**
 * The post in the team's channel: who sent it (their name broken so it pings
 * nobody) and their words, quoted. Triage reads the words back from it.
 */
export function teamPost(username: string | undefined, feedback: string): string {
  const who = username ? `@${ZWSP}${username}` : "someone";
  return `${HEAD}${who}:\n${quote(neutral(feedback))}`;
}

/** The feedback in one of the agent's team posts, or undefined if it isn't one. */
export function feedbackOfPost(content: string): string | undefined {
  if (!content.startsWith(HEAD)) return undefined;
  const body = content.split(OUTCOME)[0]!;
  const lines = body.split("\n").slice(1);
  if (!lines.length || !lines.every((l) => l.startsWith(">"))) return undefined;
  return lines.map((l) => l.replace(/^> ?/, "")).join("\n").trim();
}

/** Whether triage has already handled a team post. */
export function handled(content: string): boolean {
  return content.includes(OUTCOME);
}

/** A team post with what triage did with it ("Filed as #12: <link>"). */
export function withOutcome(content: string, outcome: string): string {
  return `${content}${OUTCOME}${outcome}`;
}

/**
 * A new issue's body: Claude's summary, then every piece of feedback as it was
 * written. Nobody's name: GitHub gets the words alone, with no pings or live
 * links ({@link forGitHub}).
 */
export function issueBody(summary: string, feedback: string[], instance: string): string {
  const said = feedback.map((f) => quote(forGitHub(f))).join("\n\n");
  const count = feedback.length === 1 ? "one piece of feedback" : `${feedback.length} pieces of feedback`;
  return (
    `${forGitHub(summary.trim())}\n\n### What people said\n\n${said}\n\n---\n\n` +
    `Grouped from ${count} sent to @${ZWSP}fuwafuwa on ${instance}.\n`
  );
}

/** A comment adding feedback to an issue that was already open. */
export function commentBody(feedback: string[], instance: string): string {
  const said = feedback.map((f) => quote(forGitHub(f))).join("\n\n");
  const more = feedback.length === 1 ? "More feedback" : `${feedback.length} more pieces of feedback`;
  return `${more} about this, sent to @${ZWSP}fuwafuwa on ${instance}:\n\n${said}\n`;
}

/** The next time triage runs: the next multiple of `hours` since midnight UTC, after `now`. */
export function nextRun(now: number, hours: number): number {
  const step = hours * 60 * 60_000;
  return (Math.floor(now / step) + 1) * step;
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
