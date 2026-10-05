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
  for an instance already pinned is refused unless its old key vouched for
  it (below), and an instance keeps at most 1000 pinned keys.

### Rotating a key

- An instance admin can **rotate** this instance's key (`RotateFederationKey`,
  on "Other instances"). In one node.db write a new key replaces the old one,
  and a **rotation** is kept: the old and new public keys and the time,
  signed by the old key over `"fuwa-federation-v1 rotation"`, then the
  origin, the old key and the new key (each after its length as 8 bytes,
  big-endian), then the time as 8 bytes. The old private key is replaced in
  node.db, and the copies this instance read of it are overwritten in
  memory (`ring` keeps its own copy inside a key pair, which goes when the
  pair does). The last 16 rotations are kept, and `GetKey`
  serves them with the key (`GetKeyResponse.rotations`).
- Then it pings every instance it pinned (and hasn't blocked), so each one
  moves now. One it couldn't reach moves the next time they talk.
- Every envelope also names the key it was signed with (`Envelope.key`),
  only as a hint. When a signed call or answer from a pinned instance fails
  its signature check but checks out with the key it names, the receiver
  fetches that instance's key again: no more than once every 5 minutes for
  one instance and named key (an admin's check looks every time), and
  within the caps on fetching strangers' keys. Envelopes forged with other
  keys get looks of their own, so they can't use up a real rotation's (at
  most 8 kept for one instance and 1024 in all at a time). It moves to the new key only if the rotations lead there
  from the key it pinned, each one signed by the key before it for that
  origin, never back to a key it already left. Then it checks the call
  again with the new key. Otherwise the call is refused as before.
- Calls signed with the old key and still on their way when the other
  instance moved are refused; their sender tries again as after any
  refusal. Nonces and the start-time check work as before.
- Two different next keys for one key, or a key going back to one it left,
  mean someone holding an old key may be moving it: nothing moves, and the
  instance is marked **check again**. Nothing goes either way with it (shares
  with it say why) until an admin here checks it again on "Other
  instances", which takes the key it has now.
- Each move is listed on "Other instances" with the instance (the last 16
  each). The log notes one fixed line, naming no instance or key.
- Rotating is routine care, not a fix for a stolen key: whoever holds the
  stolen old key can sign a rotation to a key of their own. After a theft,
  the other instances' admins check this one again and compare the new
  fingerprint over a channel they trust, as for the first pin. An instance
  pinned to a key more than 16 rotations back can't move by itself either:
  its admins check this one again.
- Stored in node migration 0022: `federation_moves` (each move followed
  here), and `needs_check` on `federation_peers`.

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
- **Rotate key**, and when it was last rotated;
- the instances this one knows: origin, fingerprint, when it was last heard
  from, whether it's blocked or needs checking again, and its key moves;
- **Blocked instances** (`federation_blocked_hosts`, field 35): host names
  this instance never calls and whose calls it turns away. Blocking one
  ends every share with it, as an instance admin ending each would: it's
  told each share ended (the only call that still goes to a blocked
  instance). Unblocking doesn't bring them back.

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
  instance's host ("Home · chat.example.com"). How this instance's own
  people look is never taken from the other: a home there can only name
  people of the guest server, shown as their own instance has them. People
  leave an instance as their id, username, display name, kind and avatar
  only.
- A guest server on another instance counts as one sender at the home: its
  people together send at most `FUWA_LIMIT_SHARED_REMOTE_SENDS_PER_MINUTE`
  messages a minute, it brings at most `FUWA_LIMIT_SHARED_REMOTE_PEOPLE`
  people (both unlimited unless set, also on the Other instances page), and
  once one of them is kept out of the channel no one new from it joins in. A home on another instance is asked about a message only in
  the channel the app named.
- A guest server on another instance that can't be reached gets three more
  tries (after 1, 4 and 15 seconds); after that, what waited for it is
  dropped, and its people see it when they next open the channel. At most
  512 events wait for one guest server, none longer than a minute. When the
  home's instance can't be reached, the guest's people are told "can't
  reach <host> right now" and nothing is sent.
- Nothing from the other instance is fetched by apps. Its pictures (servers'
  icons, people's avatars, webhooks' pictures, custom emoji, GIFs and link
  previews' pictures) are shown through the reader's own instance's picture
  proxy (`/media/outside/`), which fetches them from the other instance
  without anything about the reader. Only pictures on the other instance
  itself are taken (a link anywhere else is dropped), and an instance sends
  only its own. A link preview keeps its words, an https link and its
  pictures; threads stay with their server. Files are below.
- **Files** a guest's people send are the home's, as on one instance
  (docs/shared-channels.md), and the home's admins let a guest server send
  them with Attach Files like any other. `crate::shared_files` does it:
  - Sending: the person uploads to their own instance as anywhere else, which
    checks the file against its own caps. When the message goes to the home,
    their instance gives it a ticket for each file (`GuestSend.files`): 32
    random bytes, kept only as their SHA-256 and only in memory, for ten
    minutes, used once, and only by the home's instance (at most 4,096 kept,
    64 for one instance, the oldest going first: internal bounds). Before
    it writes the message, the home refuses any file over its own
    `FUWA_LIMIT_ATTACHMENT_UPLOAD`, its server's room for files, or the
    guest server's bytes for the day (`FUWA_LIMIT_SHARED_REMOTE_FILE_BYTES_PER_DAY`,
    unlimited unless set, counted on a placeholder account
    `shared:<server>@<instance>` that no one can sign in as). Then it fetches
    each file, `GET /federation/files/<ticket>`, signed (below), taking
    exactly the size the guest said and no more. It reads the file's kind
    from its bytes (never as the guest said), takes out a picture's
    metadata, and keeps it under its own id with its own files. The guest's
    upload is then dropped.
  - Reading: a message leaving the home carries its files by id, name, kind
    and size, never a link. The guest's instance shows each one at a link of
    its own, `/media/shared/<signature>?home=…&id=…&size=…&until=…&name=…`,
    signed by it over all of that and working for a day. Opening one, the
    guest's instance fetches the file from the home,
    `GET /federation/attachments/<server>/<id>`, signed, which the home
    answers only for a file of a message shown in a channel it shares with a
    server on that instance, now. The guest's instance reads its kind again
    from its first bytes and serves it under its own rules (pictures shown,
    audio and video played, anything else downloaded), whole (no `Range`),
    never more than the size the message said or its own
    `FUWA_LIMIT_ATTACHMENT_UPLOAD`, and never cached anywhere, so a deleted
    message's file stops at once. Apps never talk to the other instance.
  - A signed request carries a `Fuwa-Signature` header: an envelope, as
    calls use, over the request's path, from an instance whose key is pinned
    here, fresh and with a fresh nonce. Anything else, and anything the
    other side may not have, gets one plain 404, so it can't tell which
    check failed.
  - Every fetch from another instance, both ways, waits for a slot:
    `FUWA_SHARED_FILE_FETCHES_IN_FLIGHT`, 8 by default (a protective default,
    docs/capacity.md; also on the Other instances page), each instance
    getting at most half. A fetch that waits more than 30 seconds is told to
    try again. Fetches go through the same client as calls: public
    addresses only, no redirects, 15 seconds to connect and two minutes in
    all.
  - On a split instance the directory does all of this, as it keeps the
    instance key: the gateways pass `/federation/...` to it, the home's
    shard takes the bytes from it (`DirectoryService.FetchSharedFile`), and
    it takes them from the shard holding a server
    (`ShardService.SendSharedFile`, `SendSharedAttachment`).
- Stored in server migration 0020: `instance` and `instance_fingerprint` on
  `channel_guests` and `channel_links`, and `other_instances` on
  `share_codes`.

## Anonymous reports

Counts only, like every report: refused envelopes by reason (bad signature,
replay, clock, malformed), unreachable instances, refusals from the other
side, key rotations and moves, and how long calls take. Never a host name, so the reports can't say
who talks to whom.

## Next

Threads, polls and custom emoji from the other instance in shared channels.
