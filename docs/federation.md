# Federation: talking to other instances

Federation is how one fuwa instance talks to another, so servers on
different instances can share channels (phase 2 of
[shared channels](shared-channels.md)). This first part is the link itself:
each instance's key, signed calls both ways, and the settings to turn it on
and keep instances out. Sharing a channel over it comes in the next updates.

It's **off by default** (`FUWA_FEDERATION`, or "Other instances" in Instance
settings). Off, the instance answers other instances with nothing but "off".

## What never crosses

- **Apps only talk to their own instance.** Only instances talk to each
  other, so no app learns another instance's address or connects to it.
- **No one's address.** Calls between instances carry no forwarded headers,
  user agents or addresses of the person acting, and nothing about them is
  logged.
- **Only signed traffic.** Every call and every answer is signed with the
  sending instance's key and checked against the key pinned for it.

## Keys

- Each instance makes an **Ed25519 key** (with `ring`, which fuwa already
  uses) the first time it's needed, and keeps it in node.db: encrypted when
  the instance has `FUWA_ENCRYPTION_KEY`, and copied by the replica with the
  rest of node.db.
- Its **fingerprint** (SHA-256 of the public key, 8 groups of 4 hex digits)
  is on the "Other instances" page, for admins to compare with the other
  side's over a channel they trust.
- An instance is known by its **origin**, the scheme and host of its public
  URL (`https://chat.example.com`), so federation needs `FUWA_PUBLIC_URL`
  (or the Public address setting) to be https.
- An instance **pins** another's key only when one of its own admins checks
  that instance (and, from the next update, when a guest server asks with a
  share code), never because another instance said Hello. A different key
  for an instance already pinned is refused (rotation comes in a later
  update), and an instance keeps at most 1000 pinned keys.

## The wire

`fuwa.federation.v1.FederationService` (`proto/fuwa/federation/v1`) is on the
instance's public address, as gRPC-Web, like the apps' API; a split
instance's gateways pass it to the directory, which keeps the key. It isn't
in reflection: apps never use it.

- `GetKey`: the instance's origin and public key. Unsigned: the https
  connection it's fetched over vouches for the address.
- `Hello`: a signed greeting. An instance that doesn't know the caller yet
  fetches the caller's key from the caller's own address (`GetKey`) and
  checks the greeting with it, without pinning it. Greetings from unknown
  instances are capped at 60 a minute in all and 5 a minute under one
  domain, so a wildcard domain's endless names can't make an instance fetch
  without end or crowd out other instances.
- `Call`: everything else, from an instance whose key is pinned. Today its
  only call is a ping.

Each signed `Envelope` is `{from, to, sent_at_ms, nonce, reply_to, payload,
signature}`. The signature covers `"fuwa-federation-v1"` and every other
field, each after its length. The receiver checks, in order: federation is
on, the sender's origin reads and isn't blocked, it's addressed to this
instance, its time is within 5 minutes of the receiver's clock, the
signature checks out with the sender's pinned key, and its 16-byte nonce
wasn't seen in the last 10 minutes; only then does it read the payload.
Nonces are kept per sending instance (so one can't crowd out the others),
and an envelope signed before the receiving process started is refused,
since the nonces it saw before went with the last process.
Answers are signed the same way and carry the call's nonce in `reply_to`, so
an answer can't be replayed onto another call. Payloads are at most 1 MiB.

## Reaching other instances

Addresses admins type for other instances (`chat.example.com` or
`https://chat.example.com`) go through the same checks as AutoMod's own
providers: https only, no user, path or query, and never a private,
loopback, link-local or internal address, both as typed and as the name
resolves (again on each connection, so a name can't later point inside the
instance's network). No proxies and no redirects are followed; each call
has 10 seconds. `FUWA_FEDERATION_ALLOW_PRIVATE=1` (environment only) lifts
the address checks and allows plain http, for tests and private networks.

## Instance admins

On "Other instances" in Instance settings (`GetFederation`,
`CheckInstance`):

- the switch (`federation`, InstanceSettings field 34);
- this instance's origin and key fingerprint, or why its public URL can't be
  used;
- **Check an instance**: fetches and pins its key, says Hello there and
  back, and shows its fingerprint, the round trip, and whether it knows this
  instance too (a signed ping goes through once its admins have checked
  this instance);
- the instances this one knows: origin, fingerprint, when it was last heard
  from, and whether it's blocked;
- **Blocked instances** (`federation_blocked_hosts`, field 35): host names
  this instance never calls and whose calls it turns away.

## Anonymous reports

Counts only, like every report: refused envelopes by reason (bad signature,
replay, clock, malformed), unreachable instances, refusals from the other
side, and how long calls take. Never a host name, so the reports can't say
who talks to whom.

## Next

Cross-instance share codes and previews (with the home's host and
fingerprint), then messages and live events across instances, pictures
through each reader's own instance, blocking that ends shares, key rotation,
and attachments. The plan is in the shared channels phase 2 design.
