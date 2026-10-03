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
| Cloudflare Clef | `api.cloudflare.com` | An API token with Workers AI, and the account id |

and admins can add **their own**: anything at an https address that answers
the requests below. Point it at a classifier you run, or at a small service
that turns fuwa's questions into another provider's.

## What every provider gets

- Only the instance calls a provider, never an app.
- It gets the message's text and nothing else: no ids, names, servers,
  channels or addresses. Mentions become `@someone`, `@role` and `#channel`,
  custom emoji become `:emoji:`, and only the first 4,000 characters go.
  Messages in DMs and secure channels are end-to-end encrypted and never go.
- It has 3 seconds. A provider that's slow, down or answers something fuwa
  can't read lets the message through that rule; the server's other rules
  still apply. The failure is counted in the anonymous report as its kind and
  `typesafe-jev`, `cloudflare-clef` or `custom`, never your provider's name or
  address.
- fuwa never follows a redirect, and only speaks https.
- Your own provider has to be on the internet: loopback, private, link-local
  and other internal addresses (and names like `localhost` or `*.internal`)
  are refused when you save it, and a name that resolves to one isn't called.
  To run a classifier on your own network, start the instance with
  `FUWA_AUTOMOD_ALLOW_PRIVATE=1`.
- fuwa reads at most 64 KB of an answer.

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

`model` is there only when the admins typed one. The question ids are always
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

## In Rust

To build one into fuwa itself, implement `Provider` in
`server/src/automod/providers.rs`:

```rust
pub trait Provider: Send + Sync {
    /// Its id in the anonymous report.
    fn id(&self) -> &'static str;
    /// How likely `text` (already cleaned as above) is to be each label, 0 to 1.
    fn classify<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Scores, Failure>>;
}
```

then add its `Kind` to `KINDS` (id, name, host, models) and a branch for it
in `build`. `check` wraps every provider with the 3-second limit, the text
cleaning and the anonymous counts, so a provider only talks to its service.
