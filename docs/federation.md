# Federation: talking to other instances

Federation is how one fuwa instance talks to another, so servers on
different instances can share channels (phase 2 of
[shared channels](shared-channels.md)). This first part is the link itself:
each instance's key, signed calls both ways, and the settings to turn it on
and keep instances out. On top of it, a server can ask for, and be granted,
a channel from a server on another instance ([below](#sharing-a-channel-with-another-instance));
messages across instances come in the next update.

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
  that instance, or when a share is asked with a code (both instances pin
  the other's then), never because another instance said Hello. A different key
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
- `Call`: everything else, from an instance whose key is pinned: a ping, or
  a shared channel's call (`fuwa.cluster.v1.SharedCall`, the same calls the
  parts of one instance make). A share code's lookup or ask may come from an
  instance not pinned yet: its key is fetched as for `Hello`, under the same
  caps, and pinned only when it asks.

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

## Sharing a channel with another instance

- A home server's admin makes a code **for a server on another instance**
  (a switch on the channel's Share tab, shown while this instance shares
  with others). It reads `<code>@<this instance's host>`; a code made
  without the switch keeps working only on this instance.
- The guest's admin pastes it as any code. Their instance looks the code up
  at the home's instance (signed, with the home's key fetched now, pinning
  nothing) and shows the preview with **the home instance's host and key
  fingerprint**, to compare with the home's admins somewhere they trust.
- Asking pins the home's key at the guest's instance and the guest's at the
  home's, each only once the ask went through (the guest's instance once
  the home's signed answer checks out), and no more than 3 instances under
  one registered domain this way. Each server makes and takes at most 20
  lookups and asks a minute, and keeps at most 20 requests waiting from one
  other instance. The home's admins see the request with the guest instance's host
  and fingerprint before approving. Approving, turning down, ending and
  withdrawing work as on one instance.
- Each instance reads every id another instance sends as that instance's
  own: a server there is kept here as `<id>@<its host>`, and so is anyone
  acting there, so another instance can't speak for this one's servers and
  people, or a third instance's. A call to a guest is taken only from its
  home's instance, and only for a connection with that instance.
- Once approved, the guest's people read, write, edit and delete their own
  messages as on one instance: the guest's instance relays each to the
  home's, which keeps them. What's said reaches the guest's instance live,
  each message naming its author and server under the home's host; the
  guest's own people read back as its own. People's names show with their
  instance's host ("Home · chat.example.com").
- A guest server on another instance that can't be reached gets three more
  tries (after 1, 4 and 15 seconds); after that, what waited for it is
  dropped, and its people see it when they next open the channel. When the
  home's instance can't be reached, the guest's people are told "can't
  reach <host> right now" and nothing is sent.
- Nothing from the other instance is fetched by apps: its servers' icons,
  people's avatars and link previews' pictures aren't kept (pictures come
  through each instance's own proxy later), a link preview keeps only its
  words and an https link, and files can't be sent across instances yet.
- Stored in server migration 0020: `instance` and `instance_fingerprint` on
  `channel_guests` and `channel_links`, and `other_instances` on
  `share_codes`.

## Anonymous reports

Counts only, like every report: refused envelopes by reason (bad signature,
replay, clock, malformed), unreachable instances, refusals from the other
side, and how long calls take. Never a host name, so the reports can't say
who talks to whom.

## Next

Pictures through each reader's own instance, blocking that ends shares, key rotation, and attachments. The plan is in the shared channels phase 2 design.
