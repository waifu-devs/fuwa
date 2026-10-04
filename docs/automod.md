# AutoMod providers

A server's AutoMod has two kinds of rules. The ones it writes itself
(keywords, mention spam, links) run inside the instance and need no setup.
The **Smart filter** asks a moderation provider how likely a message is to be
hate, harassment, sexual content, violence, self-harm, a scam or spam, and
does what the server chose for each (nothing, flag it for the mods, block it,
or block it and time the writer out) once the provider is sure enough.

Instance admins set providers up once in **Instance settings, Moderation**,
and every server can then pick one. fuwa knows two:

| Provider | Where messages go | What it needs |
| --- | --- | --- |
| TypeSafe Jev | `api.typesafe.ai` (US) | An API key |
| Cloudflare Clef | `api.cloudflare.com` | An API token with Workers AI Read (an account token or a user token), and the account id |

and admins can add **their own**: anything at an https address that answers
the requests below. Point it at a classifier you run, or at a small service
that turns fuwa's questions into another provider's.

## What every provider gets

- Only the instance calls a provider, never an app.
- It gets the message's text and nothing else: no ids, names, servers,
  channels or addresses. Mentions become `@someone`, `@role` and `#channel`,
  custom emoji become `:emoji:`, and only the first 4,000 characters go.
  Messages in DMs and secure channels are end-to-end encrypted and never go.
- Pictures go only to providers that read them (Cloudflare Clef), and only
  when the server's Smart filter has **Check pictures too** on. See
  [Pictures](#pictures).
- Messages never wait for the provider: a message goes out at once, and the
  answer is acted on when it comes. A message it blocks is taken down then,
  with the same alert and time out, and an "AutoMod took down a message"
  entry in the audit log. The other rules always check a message before
  it's sent.
- The provider has 3 seconds to answer (6 with pictures). A provider that's slow, down or
  answers something fuwa can't read lets the message through that rule; the
  server's other rules still apply. The failure is counted in the anonymous report as its kind and
  `typesafe-jev`, `cloudflare-clef` or `custom`, never your provider's name or
  address.
- fuwa sends each provider as many checks at once as it keeps up with:
  one more after each quick answer, and half as many after a timeout, a
  refusal or an error. Other checks wait their turn. When 256 are already
  waiting, a check isn't sent, and the message goes through that rule
  unchecked. The same message sent over and over in one server (a raid, a
  spammer) is asked about once while that check is out.
- fuwa never follows a redirect, and only speaks https.
- Your own provider has to be on the internet: loopback, private, link-local
  and other internal addresses (and names like `localhost` or `*.internal`)
  are refused when you save it, and a name that resolves to one isn't called.
  To run a classifier on your own network, start the instance with
  `FUWA_AUTOMOD_ALLOW_PRIVATE=1`.
- fuwa reads at most 64 KB of an answer.
- Admins can cap how many times a day (UTC) each server's Smart filter asks
  its provider (Instance settings, Moderation, or
  `FUWA_LIMIT_AUTOMOD_CHECKS_PER_DAY`); there's no cap unless set. Past it,
  that server's messages go through the Smart filter unchecked until
  midnight UTC, like when the provider is down, and its other rules still
  apply: a limit that blocked messages would stop a busy server talking. Each
  server's Usage page shows the day's count. The first time a server runs
  out on a day, AutoMod posts one alert in its Smart filter rule's alert
  channel (when the rule alerts) saying so; it names no message or member.
  It's counted where the server lives, in memory, so a restart starts the
  day's count again (and may post that day's alert again).

## The request

One `POST` per message, `Content-Type: application/json`. The key, when
there is one, goes as `Authorization: Bearer <key>`, or in the header you
named (`X-Api-Key: <key>`). This is the System One decision format that Jev
and Clef speak:

```json
{
  "model": "your-model",
  "state": { "chat_message": "free nitro, log in at discord-gift.example" },
  "questions": {
    "hate": {
      "type": "noul",
      "instructions": "Does this chat message attack, demean or dehumanize people for who they are (...)?",
      "criteria": {
        "true": "It attacks or demeans people for who they are, including slurs aimed at someone.",
        "false": "It doesn't. Quoting, reporting on or discussing hate without endorsing it doesn't count."
      }
    },
    "harassment": { "type": "noul", "instructions": "...", "criteria": { "true": "...", "false": "..." } },
    "sexual":     { "...": "..." },
    "violence":   { "...": "..." },
    "self_harm":  { "...": "..." },
    "scam":       { "...": "..." },
    "spam":       { "...": "..." }
  }
}
```

`model` is there only when the admins typed one. For Clef, fuwa sends to
`https://api.cloudflare.com/client/v4/accounts/<account id>/ai/run/@cf/cloudflare/clef`
(or `clef-flash`) with the bare `"model": "clef"` in the body, and the token
as `Authorization: Bearer`, which works the same for account-owned and user
tokens. The question ids are always
these seven; the wording may get better between releases, so read the ids,
not the text.

## The answer

`200` with a probability from 0 to 1 that the answer is yes, for every
question:

```json
{
  "answers": {
    "hate":       { "type": "noul", "noul": 0.01 },
    "harassment": { "type": "noul", "noul": 0.02 },
    "sexual":     { "type": "noul", "noul": 0.0 },
    "violence":   { "type": "noul", "noul": 0.0 },
    "self_harm":  { "type": "noul", "noul": 0.0 },
    "scam":       { "type": "noul", "noul": 0.97 },
    "spam":       { "type": "noul", "noul": 0.64 }
  }
}
```

fuwa also reads `"probability": 0.97` or `"probabilities": { "true": 0.97 }`
in place of `"noul"`, and an answer wrapped as `{ "result": { "answers": ... } }`
(Cloudflare's way). A missing question, or a number outside 0 to 1, counts as
no answer.

Anything else is a failure, and the admins' **Test connection** shows it:

| Status | Read as |
| --- | --- |
| `401`, `403` | The key was turned down |
| `429` | Too many requests |
| Anything else | It failed; `{"error": {"message": "..."}}` or `{"errors": [{"message": "..."}]}` is shown, cut to 160 characters |

## Pictures

With **Check pictures too** on, Clef also gets up to 4 of a message's
pictures: attached ones (by their type or name), then embeds' images and
thumbnails. The instance fetches them itself, the way it fetches every
picture from elsewhere for readers: public addresses only, at most 8 MB, 2
seconds in all. Uploads on the instance are read from its own files. It keeps
PNG, JPEG and WebP of at most 4 MiB and 16 megapixels each, 8 MiB in all,
and leaves out the rest (a GIF, a huge photo, one that didn't arrive in
time). They go in the request as data URIs, the way Clef takes them:

```json
{
  "state": { "chat_message": "look at this" },
  "images": ["data:image/png;base64,iVBORw0KGgo..."],
  "questions": { "...": "..." }
}
```

and each question then also says to count the pictures as part of the
message. A message that's only pictures is asked about too. Edits are asked
about by their text alone, since their pictures can't change. The admins' own
providers get text only.

## In Rust

To build one into fuwa itself, implement `Provider` in
`server/src/automod/providers.rs`:

```rust
pub struct Message<'a> {
    /// The text, already cleaned as above.
    pub text: &'a str,
    /// Its pictures, when the rule shows them and the provider reads them.
    pub pictures: &'a [Picture],
}

pub trait Provider: Send + Sync {
    /// Its id in the anonymous report.
    fn id(&self) -> &'static str;
    /// How likely the message is to be each label, 0 to 1.
    fn classify<'a>(&'a self, message: &'a Message<'a>) -> BoxFuture<'a, Result<Scores, Failure>>;
}
```

then add its `Kind` to `KINDS` (id, name, host, models, and `pictures: true`
if it reads them) and a branch for it in `build`. `check` wraps every
provider with the time limit, the text cleaning and the anonymous counts, so
a provider only talks to its service.
