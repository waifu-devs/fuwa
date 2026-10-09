import assert from "node:assert/strict";
import { test } from "node:test";
import {
  FeedbackPace,
  asksForHelp,
  commentBody,
  defanged,
  feedbackOf,
  feedbackOfPost,
  handled,
  issueBody,
  neutral,
  nextRun,
  perHourOf,
  teamPost,
  withOutcome,
} from "../src/feedback.ts";
import { GitHub } from "../src/github.ts";

const me = { id: "01JAGENT", username: "fuwafuwa" };

test("feedback leaves out the agent's own mentions", () => {
  assert.equal(feedbackOf("<@01JAGENT> the picker is slow", me), "the picker is slow");
  assert.equal(feedbackOf("@fuwafuwa: the picker is slow", me), "the picker is slow");
  assert.equal(feedbackOf("hey @FuwaFuwa, dark mode please", me), "hey dark mode please");
  assert.equal(feedbackOf("ask @fuwafuwa2 about it", me), "ask @fuwafuwa2 about it");
  assert.equal(feedbackOf("mail me@fuwafuwa.dev", me), "mail me@fuwafuwa.dev");
});

test("an empty mention or a hello asks for help", () => {
  assert.ok(asksForHelp(feedbackOf("<@01JAGENT>", me)));
  assert.ok(asksForHelp("help"));
  assert.ok(asksForHelp("Hi"));
  assert.ok(!asksForHelp("help, uploads fail"));
});

test("quoted feedback names nobody and pings nobody", () => {
  const text = neutral("<@01JPERSON> and <@&01JROLE> in <#01JCHAN> :) <:blob:01JEMOJI> @octocat @everyone");
  assert.ok(!text.includes("01J"));
  assert.ok(text.includes(":blob:"));
  assert.ok(!/@[\w-]/.test(text));
});

test("links are broken so GitHub neither links nor loads them", () => {
  assert.equal(defanged("see https://a.example/x and HTTP://b.example"), "see hxxps[:]//a.example/x and Hxxp[:]//b.example");
  assert.equal(defanged("ftp://c.example"), "ftp[:]//c.example");
  assert.equal(defanged("![x](//d.example/p.png) <img src=\"//e.example\">"), "![x]([/]/d.example/p.png) <img src=\"[/]/e.example\">");
  assert.equal(defanged("www.f.example"), "www[.]f.example");
  assert.equal(defanged("a/b // comment, 1:2"), "a/b // comment, 1:2", "ordinary text stays as it was");
});

test("each account sends so much feedback an hour, and hears so once", () => {
  const pace = new FeedbackPace(2);
  const h = 60 * 60_000;
  assert.equal(pace.take("mika", 0), "ok");
  assert.equal(pace.take("mika", 1), "ok");
  assert.equal(pace.take("mika", 2), "over", "a third in the hour is one too many");
  assert.equal(pace.take("mika", 3), "quiet", "and it's said once, not to every one after");
  assert.equal(pace.take("rin", 2), "ok", "each account has its own");
  assert.equal(pace.take("mika", h), "ok", "an hour after the first, there's room for one");
  assert.equal(pace.take("mika", h), "quiet", "the second is still within the hour, and so is the telling");
  assert.equal(pace.take("mika", h + 2), "ok");
  assert.equal(pace.take("mika", h + 3), "over", "told again once the last telling is an hour old");
  const open = new FeedbackPace(Infinity);
  for (let i = 0; i < 100; i++) assert.equal(open.take("mika", i), "ok");
});

test("FEEDBACK_PER_HOUR is 5 unless set, a whole number or unlimited", () => {
  assert.equal(perHourOf(undefined), 5);
  assert.equal(perHourOf(" "), 5);
  assert.equal(perHourOf("30"), 30);
  assert.equal(perHourOf("Unlimited"), Infinity);
  for (const bad of ["0", "-1", "5/h", "1.5", "1e3", "lots"]) assert.equal(perHourOf(bad), undefined, bad);
});

test("a team post gives its feedback back, until triage handles it", () => {
  const post = teamPost("juan", "uploads fail\n\non <@01JX> Firefox");
  assert.ok(post.startsWith("Feedback from @\u200bjuan:\n> uploads fail\n>\n> on @\u200bsomeone Firefox"));
  assert.equal(feedbackOfPost(post), "uploads fail\n\non @\u200bsomeone Firefox");
  assert.ok(!handled(post));
  const done = withOutcome(post, "Filed as #3: https://github.com/waifu-devs/fuwa/issues/3");
  assert.ok(handled(done));
  assert.equal(feedbackOfPost(done), feedbackOfPost(post));
  assert.equal(feedbackOfPost("Thanks, got it!"), undefined);
  assert.equal(teamPost(undefined, "x"), "Feedback from someone:\n> x");
});

test("issues and comments quote every piece and name nobody", () => {
  const body = issueBody("Uploads fail on Firefox.", ["one", "two\nlines"], "fuwa.chat");
  assert.ok(body.startsWith("Uploads fail on Firefox.\n\n### What people said\n\n> one\n\n> two\n> lines"));
  assert.ok(body.includes("2 pieces of feedback sent to @\u200bfuwafuwa on fuwa.chat"));
  assert.ok(commentBody(["more"], "fuwa.chat").startsWith("More feedback about this"));
});

test("triage runs on the hour, every few hours from midnight UTC", () => {
  const h = 60 * 60_000;
  assert.equal(nextRun(0, 6), 6 * h);
  assert.equal(nextRun(6 * h, 6), 12 * h);
  assert.equal(nextRun(6 * h - 1, 6), 6 * h);
  assert.equal(nextRun(25 * h, 6), 30 * h);
});

test("GitHub gets the label, and the token stays out of errors", async () => {
  const seen: { url: string; init: RequestInit }[] = [];
  const gh = new GitHub({
    repo: "waifu-devs/fuwa",
    token: "secret-token",
    label: "feedback",
    fetch: (async (url: string, init: RequestInit) => {
      seen.push({ url, init });
      if (init.method === "GET") {
        return Response.json([
          { number: 1, title: "an issue", body: null },
          { number: 2, title: "a pull request", body: "", pull_request: {} },
        ]);
      }
      return Response.json({ number: 7, html_url: "https://github.com/waifu-devs/fuwa/issues/7" }, { status: 201 });
    }) as typeof fetch,
  });
  assert.deepEqual(await gh.createIssue("t", "b"), { number: 7, url: "https://github.com/waifu-devs/fuwa/issues/7" });
  assert.equal(seen[0]!.url, "https://api.github.com/repos/waifu-devs/fuwa/issues");
  assert.deepEqual(JSON.parse(seen[0]!.init.body as string), { title: "t", body: "b", labels: ["feedback"] });
  assert.deepEqual(await gh.openIssues(), [{ number: 1, title: "an issue", body: "" }]);
  assert.match(seen[1]!.url, /\/issues\?state=open&labels=feedback&per_page=100$/);
  await gh.comment(1, "more");
  assert.equal(seen[2]!.url, "https://api.github.com/repos/waifu-devs/fuwa/issues/1/comments");

  const failing = new GitHub({
    repo: "waifu-devs/fuwa",
    token: "secret-token",
    label: "feedback",
    fetch: (async () => new Response("no", { status: 401 })) as typeof fetch,
  });
  await assert.rejects(failing.createIssue("t", "b"), (err: Error) => !err.message.includes("secret-token"));
  assert.throws(() => new GitHub({ repo: "nope", token: "x", label: "feedback" }));
});
