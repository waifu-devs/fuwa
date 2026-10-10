# Agent endpoints

An agent can get its events over HTTP instead of holding a stream open:
its owner gives it an endpoint, a URL the instance posts the agent's events
to. That suits agents that run only when called (Cloudflare Workers, Lambda,
a small web app) and keep no state between calls. Instances that have this
list `agent-endpoints` in `Node.versions.features`.

Everything is `fuwa.v1.AgentService` (`proto/fuwa/v1/agent.proto`) plus
the instance setting `agent_endpoints`.

## Setting one up

Only the agent's owner (a person, never an agent) manages its endpoint:

- `GetAgentEndpoint(agent_id)` gives the endpoint. The first time it's asked
  for, the endpoint is made (off, with no URL) with a new signing secret,
  and that answer is the only one with the secret in it, so the agent can be
  set up to check signatures before it has a URL. Later answers say only
  that there is one (`secret_set`) and its last four characters
  (`secret_hint`); a lost secret is replaced, not shown again.
- `SetAgentEndpoint(agent_id, url, events)` sets the URL and which events go
  there. `events` names payloads of `Event` (`message_created`,
  `interaction_created`, `member_joined`...); empty sends all of them. Before
  it saves, the instance sends a check (below); a URL that doesn't pass isn't
  saved. An empty URL turns the endpoint off. Every successful call starts
  deliveries again from the events after it, in every server.
- `ResetAgentEndpointSecret(agent_id)` makes a new secret, used from then on,
  and gives it this once.

The token keeps working alongside: an agent with an endpoint can still call
the API (and still has to, for anything but answering interactions).

## What's posted

A `POST` to the URL with a `fuwa.v1.AgentDelivery` in proto3 JSON
(`content-type: application/json`, lowerCamelCase field names, 64-bit
numbers as strings, timestamps as RFC 3339):

```json
{
  "agentId": "01J...",
  "events": [
    {
      "id": "01J...",
      "serverId": "01J...",
      "sequence": "1042",
      "actorId": "01J...",
      "createdAt": "2026-10-09T12:00:00Z",
      "interactionCreated": { "interaction": { "id": "01J...", "command": "roll", "...": "..." } }
    }
  ]
}
```

- One delivery holds one server's events, in order, up to 50 of them; each
  server goes on at its own pace, so events of different servers can arrive
  out of order with each other.
- The events are what `EventService.Subscribe` would send the agent: only
  channels it can see, `InteractionCreated` only for itself and with its
  arguments, channels appearing and going as its permissions change (those
  have sequence 0 and come right after the event that caused them).
- Events that aren't stored aren't delivered: voice states and live tiles
  (list them with the API).
- Any `2xx` means the delivery arrived; anything else, or no answer within 10
  seconds, is a failure, and redirects aren't followed. A failed delivery is
  tried again, from 1 second up to every 5 minutes, and may come back with
  more events after it, so skip events whose `(serverId, sequence)` you've
  already had.
- An endpoint that has failed for a day is turned off (`disabled_at`), and
  its owner sees why (`last_error`). Setting the URL again turns it back on;
  what happened meanwhile isn't sent (`EventService.ListEvents` has it).

When an agent leaves a server (or is removed, or the server is deleted), its
last delivery from there carries that, and nothing more comes from it.

### Signatures

Every delivery is signed the [Standard Webhooks](https://www.standardwebhooks.com)
way, so its libraries check it as they are:

- `webhook-id`: the delivery's id (its first event's).
- `webhook-timestamp`: when it was sent, in unix seconds.
- `webhook-signature`: `v1,` and the base64 HMAC-SHA256 of
  `<webhook-id>.<webhook-timestamp>.<body>`, keyed with the secret's bytes
  (the base64 after `whsec_`).

Check the signature over the body exactly as it came, and refuse timestamps
more than a few minutes off.

### The check

When the URL is set, the instance posts a delivery with no events and a
`challenge`, signed like any other. The endpoint must answer `2xx` with:

```json
{ "challenge": "<the same challenge>" }
```

Only something written for fuwa does that, so nobody can point an agent at
someone else's site.

## Answering interactions

The answer to a delivery may carry replies to the interactions in it, as a
`fuwa.v1.AgentDeliveryAnswer`:

```json
{
  "replies": [
    {
      "interactionId": "01J...",
      "content": "You rolled a 4",
      "components": [{ "buttons": [{ "customId": "again", "label": "Roll again" }] }]
    }
  ]
}
```

Each is posted as if the agent had called `SendMessage` with
`interaction_id` in the interaction's channel: its permissions, slow mode
and AutoMod apply, and the usual 5 answers in 15 minutes. A refused reply is
dropped (it doesn't fail the delivery). Answering later, or anything else,
is the API with the agent's token.

## Where endpoints may be

The instance setting `agent_endpoints` (`FUWA_AGENT_ENDPOINTS`, Instance
settings, Sign-ups → Agents) decides:

- `public` (the default): `https` at public addresses only. Names are
  checked as they're looked up, like the pictures the instance fetches, so a
  name can't point the instance at itself or its network.
- `any`: `http` or `https` anywhere, the instance's own machine and network
  included: for an agent running next to a self-hosted instance.
- `off`: no endpoints. Deliveries wait where they are until it's back on.

## On a split instance

Endpoints live in the directory's node.db (`agent_endpoints`). Deliveries
run on the shard keeping each server (`api/endpoints.rs`), which asks the
directory for endpoints (`AgentEndpoints`), remembers them until the
directory's watch says one changed, and reports how deliveries go
(`ReportAgentDelivery`). Where each agent stands in a server's log is that
server's file (`agent_deliveries`), so it moves with the server.

## Privacy

Deliveries carry only what the agent could read through the API, to the URL
its owner set. URLs and secrets are never logged; the instance logs only
that an endpoint was set or failed.
