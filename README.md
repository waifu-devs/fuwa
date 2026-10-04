# fuwa

A self-hostable, Discord-like chat server. Run it yourself with one command, or
use the instance Waifu Devs hosts; clients connect to as many instances as they
like, hosted or self-hosted, over the same protocol.

- **One database per community server.** Every server lives in its own
  [Turso](https://github.com/tursodatabase/turso) (SQLite-compatible) file, so a
  server can be backed up or moved by copying one file. Writes use Turso's
  concurrent writes, so messages to the same server land in parallel.
- **Accounts your way.** Standalone accounts (a username and password kept on the
  instance) work with no dependency on anyone. Linked accounts sign in with a
  waifu.dev account (see [Signing in with waifu.dev](#signing-in-with-waifudev)).
  The operator switches each kind on or off.
- **Each server picks its door.** Anyone on the instance from Browse, or only
  people with an invite; rules new members agree to before they talk;
  applications, with questions, that someone looks over before letting people
  in; waifu.dev accounts only; a minimum account age. All of it is set from
  the app.
- **Direct messages only the two of you can read.** Every direct message is
  end-to-end encrypted with [MLS](https://www.rfc-editor.org/rfc/rfc9420)
  (RFC 9420, through [OpenMLS](https://github.com/openmls/openmls)). Each
  device has its own keys, which never leave it; the instance only stores and
  passes along ciphertext, and can't read it even if its operator wanted to.
  There's no off switch. See [docs/e2ee.md](docs/e2ee.md).
- **Secure channels.** A server's admins can make channels that are end-to-end
  encrypted the same way: only the people the channel's permissions let in can
  read them, on their own devices, and the server can't. See
  [docs/secure-channels.md](docs/secure-channels.md).
- **Channels shared between servers.** Two communities can talk in one
  channel, like Slack Connect: both admins agree, the messages live only on
  the server that shared it, and each side keeps its own roles and
  moderators. See [docs/shared-channels.md](docs/shared-channels.md).
- **Profile effects.** Petals, starfall, sparkles, confetti and more over
  your profile card: a short intro when it opens, then a gentle loop, drawn
  by the app in your theme's colors. Viewers and instances can turn them off.
  See [docs/profile-effects.md](docs/profile-effects.md).
- **GIFs, privately.** GIF search (GIPHY or Klipy) goes through the instance,
  so the library never sees who's searching, and a sent GIF is stored on the
  instance. See [docs/gifs.md](docs/gifs.md).
- **Usage tracked, limits optional.** Every server counts its members, channels,
  messages and storage. Limits are off unless the operator sets them.
- **Live by design.** Every change is an event in the server's log; clients
  follow the log live and catch up from their last sequence after a reconnect.
- **One app for every instance.** Each instance serves the fuwa web app at its
  own address, and there's a desktop app for Windows, macOS and Linux (native
  Rust, drawn with [GPUI Kit](https://gpui-kit.com)). Both keep a list of the
  instances you've added, hosted or self-hosted, and show all their servers
  side by side.
- **One binary, split when you need to.** By default one process does
  everything. The same binary can run as gateways, a directory and shards
  instead, to spread a big instance across machines (see
  [Scaling out](#scaling-out)).

## Self-host

```sh
docker run -d --name fuwa -p 8080:8080 -v fuwa:/data \
  -e FUWA_PUBLIC_URL=https://chat.example.com \
  ghcr.io/waifu-devs/fuwa:0.1
```

Then open the address in a browser: the web app is served from the same port.
[The self-hosting guide](docs/self-hosting.md) goes step by step: a domain
with https, the binary as a service, Railway, updates and backups.

Images are tagged by release (`0.1` follows the newest 0.1.x, `0.1.0` is that
release, `latest` follows master) and run on x86 and ARM. Each
[release](https://github.com/waifu-devs/fuwa/releases) also has the binary for
Linux (x86 and ARM), macOS and Windows.

Or build and run the binary. The web app is compiled in with the `web` feature,
after building it once:

```sh
(cd web && pnpm install && pnpm build)
cargo run --release --features web --bin fuwa
```

Without `--features web` the binary serves only the API.

The first account to sign up becomes the instance's admin. Put it behind a
reverse proxy with TLS for anything public; the server speaks plain HTTP/1.1
and HTTP/2.

### Checking the image

Every published image comes with a signed record of the commit and the
workflow that built it, so you can check that what you pulled is an official
build:

```sh
gh attestation verify oci://ghcr.io/waifu-devs/fuwa:latest --repo waifu-devs/fuwa
```

Builds are also reproducible: building the same commit gives the same binary,
byte for byte, so you can rebuild an image's binary yourself and compare:

```sh
git checkout <commit>
docker build --target binary --build-arg FUWA_COMMIT=$(git rev-parse HEAD) --output type=local,dest=out .
docker cp $(docker create ghcr.io/waifu-devs/fuwa:sha-<short commit>):/usr/local/bin/fuwa published
sha256sum out/fuwa published
```

The instance reports the commit it was built from in `NodeService.GetNode`,
and the app shows it next to the version.

### Configuration

The instance starts from environment variables (a `.env` file in the working
directory also works; real environment variables win). Instance admins can
then change the name, public URL, allowed origins, accounts, server creation,
default limits, the usage signal and the web app from the app's instance
settings (or `AdminService.UpdateSettings`). Those changes are stored in
`node.db`, apply at once and survive restarts; the variables stay as the
defaults underneath, and resetting a setting returns to them. The data path,
host, port, encryption key, admin token, telemetry URL, `FUWA_HOSTING` and
the log filter are read only from the environment.

| Variable | Default | What it does |
| --- | --- | --- |
| `FUWA_DATA_PATH` | `~/.fuwa` (`/data` in Docker) | Where the databases live |
| `FUWA_HOST` | `0.0.0.0` | Address to listen on (`::` for IPv6 and IPv4 both) |
| `FUWA_PORT` | `PORT`, else `8080` | Port to listen on |
| `FUWA_PUBLIC_URL` | `http://localhost:<port>` | The URL clients reach this instance on; uploaded pictures are linked through it |
| `FUWA_NODE_NAME` | `Fuwa` | The instance's display name |
| `FUWA_ALLOWED_ORIGINS` | `*` | Browser origins allowed to call the API, comma-separated |
| `FUWA_WEB` | `on` | Serve the web app at `/`; `off` leaves only the API |
| `FUWA_LOCAL_ACCOUNTS` | `open` | Standalone accounts: `open` (anyone can sign up), `closed` (existing accounts only), `off` |
| `FUWA_LINKED_ACCOUNTS` | `open` | Signing in with waifu.dev: `open` (anyone with a waifu.dev account gets one here), `closed` (existing linked accounts only), `off` |
| `FUWA_LINKED_ISSUER` | `https://api.waifu.dev` | The OpenAuth issuer linked accounts sign in with |
| `FUWA_SSO_ACCOUNTS` | `off` | Single sign-on through the identity provider set up in instance settings: `open`, `closed` (existing SSO accounts only), `off` |
| `FUWA_SERVER_CREATION` | `everyone` | Who can create servers: `everyone`, `admins`, `off` |
| `FUWA_AGENT_CREATION` | `everyone` | Who can make agents (accounts programs drive): `everyone`, `admins`, `off` |
| `FUWA_SHARED_CHANNELS` | `on` | Servers sharing a text channel with another server on this instance ([docs/shared-channels.md](docs/shared-channels.md)); `off` stops new shares |
| `FUWA_MCP` | `on` | Agents using the instance through MCP at `/mcp` with their token ([docs/mcp.md](docs/mcp.md)); `off` turns the endpoint off |
| `FUWA_PROFILE_EFFECTS` | `on` | People putting an animated effect on their profile card ([docs/profile-effects.md](docs/profile-effects.md)); `off` hides everyone's |
| `FUWA_RICH_PRESENCE` | `on` | People may show what they're doing (games and apps) to people they share a server with, once they turn it on themselves ([docs/presence.md](docs/presence.md)); `off` drops every activity, statuses still show |
| `FUWA_FEDERATION` | `off` | Talking to other fuwa instances with signed calls, for sharing channels across instances ([docs/federation.md](docs/federation.md)); needs an https `FUWA_PUBLIC_URL` |
| `FUWA_FEDERATION_ALLOW_PRIVATE` | `off` | Lets federation reach private, loopback and internal addresses and plain http, for tests and private deployments |
| `FUWA_GIF_PROVIDER` | `off` | GIF search: `giphy`, `klipy` or `off` ([docs/gifs.md](docs/gifs.md)); instance settings can change it later |
| `FUWA_GIF_API_KEY` | unset | The GIF provider's key; GIFs stay off without one |
| `FUWA_ADMIN_TOKEN` | unset | A bearer token with instance-admin rights, for scripts or a control plane (32+ characters) |
| `FUWA_ENCRYPTION_KEY` | unset | 64 hex characters (`openssl rand -hex 32`); encrypts every database at rest |
| `FUWA_LIMIT_SERVERS_PER_ACCOUNT` | unlimited | Servers one account may own |
| `FUWA_LIMIT_MEMBERS` | unlimited | Members per server |
| `FUWA_LIMIT_CHANNELS` | unlimited | Channels per server |
| `FUWA_LIMIT_STORAGE` | unlimited | Database size per server, like `500MB` or `2GiB` |
| `FUWA_LIMIT_ATTACHMENT_STORAGE` | unlimited | Uploaded files per server, custom emoji included |
| `FUWA_LIMIT_RECORDING_STORAGE` | unlimited | Voice channel recordings kept on the server, per server, e.g. `20GB` |
| `FUWA_LIMIT_EMOJIS` | unlimited | Custom emoji per server |
| `FUWA_LIMIT_PICTURE_UPLOAD` | unlimited | Largest avatar, banner, server icon or emoji one upload may be, like `8MB` |
| `FUWA_LIMIT_PICTURE_UPLOADS_PER_DAY` | unlimited | Pictures one account may upload in a day (UTC), like `256MiB` |
| `FUWA_LIMIT_VOICE_MESSAGE_SECONDS` | unlimited | The longest voice message; apps stop recording there. The instance can only check the length apps report, not the audio itself |
| `FUWA_LIMIT_VOICE_MESSAGE_BYTES` | unlimited | The biggest voice message, like `2MiB` (in direct messages, encrypted and padded) |
| `FUWA_LIMIT_VOICE_MESSAGES_PER_DAY` | unlimited | Voice messages one account may upload in a day (UTC), apart from pictures |
| `FUWA_LIMIT_ATTACHMENT_UPLOAD` | unlimited | Largest file one message may carry, like `100MB` |
| `FUWA_LIMIT_ATTACHMENT_UPLOADS_PER_DAY` | unlimited | Files one account may send in a day (UTC), apart from pictures, like `2GiB` |
| `FUWA_LIMIT_AUTOMOD_CHECKS_PER_DAY` | unlimited | Times a day (UTC) one server's Smart filter may ask its moderation provider; past it, messages go through the Smart filter unchecked (also set from the app: Instance settings, Moderation) |
| `FUWA_LIMIT_SHARED_REMOTE_SENDS_PER_MINUTE` | unlimited | Messages a minute all the people of one server on another instance may send together to channels shared from here (also set from the app: Instance settings, Other instances) |
| `FUWA_LIMIT_SHARED_REMOTE_PEOPLE` | unlimited | People one server on another instance may bring to a server's shared channels; past it, no one new from that server joins in (also set from the app: Instance settings, Other instances) |
| `FUWA_TELEMETRY` | `on` | The anonymous usage signal and health reports; `off` turns both off (so does `DO_NOT_TRACK=1`), and apps on the instance then send no reports either |
| `FUWA_TELEMETRY_URL` | `https://analytics.waifu.dev/v1/fuwa/signals` | Where the signal goes |
| `FUWA_REPORTS_URL` | `FUWA_TELEMETRY_URL` with `/signals` changed to `/reports` | Where the hourly health report goes |
| `FUWA_UPDATE_CHECK` | `on` | Asks GitHub daily whether a newer fuwa is out, to tell admins (instance settings) and pass desktop apps their updates; never installs anything ([Updating](docs/self-hosting.md#updating)) |
| `FUWA_HOSTING` | `self_hosted` | `hosted` only on Waifu Devs' own instance; reported in the signal |
| `FUWA_LOG` | `info,turso_core=warn` | Log filter ([syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)) |
| `FUWA_CALLS` | `on` | Voice channels and calls in direct messages; `off` turns them off |
| `FUWA_CALL_RECORDINGS` | `on` | Recording voice channels on the server (a track per person, for people with Record); `off` turns it off |
| `FUWA_CALL_RECORDINGS_KEEP_DAYS` | unset | Days a finished server recording is kept before it deletes itself; unset keeps them until someone does |
| `FUWA_MEDIA_PORT` | `50000` | The port calls' sound uses, UDP and TCP, open to the internet; `off` for no calls in this process |
| `FUWA_MEDIA_ADDRESSES` | this machine's address | Where apps reach that port: `HOST`, `HOST:PORT`, or `udp/…` or `tcp/…` for one protocol (a TCP proxy), comma-separated |
| `FUWA_ICE_URLS` | unset | STUN and TURN servers for people on strict networks, comma-separated `stun:`, `turn:` and `turns:` URLs |
| `FUWA_TURN_SECRET` | unset | The TURN servers' shared secret (coturn's `static-auth-secret`); each call gets a password from it |
| `FUWA_JEV_API_KEY` | unset | Turns on TypeSafe Jev for servers' AutoMod smart filter (also set from the app: Instance settings, Moderation) |
| `FUWA_AUTOMOD_ALLOW_PRIVATE` | off | `1` lets instance admins add a moderation provider of their own at a private or internal address (your own network); off, those are refused when saved and when called |
| `FUWA_CLEF_API_TOKEN`, `FUWA_CLEF_ACCOUNT_ID` | unset | Turn on Cloudflare Clef (Workers AI) the same way; set both |
| `FUWA_S3_BUCKET` and the other `FUWA_S3_*` | unset | Split instances only: a bucket the directory and shards copy their files to as they change; see [Replicating to a bucket](#replicating-to-a-bucket) |
| `FUWA_REPLICA_PATH` | unset | Split instances only: a folder to replicate to instead of a bucket |
| `FUWA_RESTORE` | `off` | `if-empty`: restore a part from the replica when its data folder is empty |

The `FUWA_LIMIT_*` values are instance-wide defaults. An admin can give a single
server its own caps from that server's settings or the Servers page of the
instance settings (or `AdminService.SetServerLimits`); a server's own caps win
over the defaults.

### Signing in with waifu.dev

People can sign in with their waifu.dev account instead of making a password
here. The first time, they get a linked account on the instance, named after
their waifu.dev username (with a number added if it's taken here), with their
waifu.dev name and picture to start with. It needs `FUWA_PUBLIC_URL` to be the
https address people use (or `http://localhost` while testing), because that's
where waifu.dev sends them back to: the instance is its own OpenAuth client,
with its public URL as the client ID and `<FUWA_PUBLIC_URL>/auth/waifu/callback`
as the only address the sign-in can return to.

How it goes:

1. The app calls `AuthService.StartLinkedSignIn` with its own origin and the
   SHA-256 of a secret it keeps, and sends the browser to the `authorize_url`
   it gets back (a code flow with PKCE; the verifier stays on the instance).
2. waifu.dev sends the browser to the instance's `/auth/waifu/callback` with a
   code. When another fuwa app (on another address) started the sign-in, this
   page asks the person to confirm it's theirs, then hands the code to that
   app's own `/auth/waifu/callback`.
3. The app calls `FinishLinkedSignIn` with the code and its secret. The
   instance trades the code for a token, asks waifu.dev's `/userinfo` who signed
   in, and answers with a session. The token is made out to the instance and
   waifu.dev takes it for nothing else; the instance hands its refresh token
   straight back.

The callback page is served even with `FUWA_WEB=off`, so apps on other
addresses can still sign in to the instance. Turning standalone accounts off
needs waifu.dev sign-in (or single sign-on) working first, and the other way
around, so there's always a way in.

A community server can take waifu.dev accounts only (Access, in its settings):
people with an account made on the instance can't join or apply there, though
members who are already in stay.

### Single sign-on (SAML and OpenID Connect)

An organization's identity provider (Okta, Microsoft Entra ID, Google
Workspace, Keycloak, Authentik...) can sign people in at two levels, each off
until someone sets it up, with the same settings form for both:

- **The instance**: an admin picks OpenID Connect or SAML 2.0 under instance
  settings, Single sign-on, and opens SSO accounts (`FUWA_SSO_ACCOUNTS` is only
  the default). People get a "Continue with <name>" button, and an account
  here the first time. **Test sign-in** checks the saved provider without
  signing anyone in.
- **One community server**: its owner (only the owner, since the provider
  decides who gets in) sets up a provider, at an https address, under the
  server's settings, Single sign-on. Once it's required, people sign
  in through it to join (or apply), and again every 7, 30 or 90 days (or never)
  to keep seeing the server. Members whose sign-in is missing or ran out stay
  members but see no channels until they sign in; the owner and agents never
  need to. Changing which provider it is (its issuer or entity ID, client ID,
  sign-in URL or signing certificates) forgets every sign-in and stops
  requiring it, and the audit log records each of those, certificates by
  SHA-256 fingerprint. Join buttons and the locked screen name the provider's
  host, since its sign-in page sees the address of whoever signs in. Who has
  signed in, and when, is shown only to that member and to managers.

Both can list email domains: only people whose verified email is on one of
them get in. A sign-in to the instance keeps nothing on it until the provider
answers: its state is signed and carries what's needed, so nobody can fill
the instance with sign-ins that never finish, and each state works once. A
server's sign-ins are capped per 10 minutes (1,000 through its provider, 10
by one person), counted without ever looking at client addresses. A signed
SAML response must name this instance as its Destination. The sign-in
buttons name the provider's host, whose page sees your IP address. The
settings page shows what to tell the provider (the OIDC
redirect URI, or the SAML entity ID, which is also the metadata URL, and the
Assertion Consumer Service). Everything comes back to this instance's public
address, so `FUWA_PUBLIC_URL` has to be its https address (or `http://localhost`
while testing):

| | Instance | Server |
| --- | --- | --- |
| OIDC redirect URI | `/sso/instance/oidc` | `/sso/servers/<id>/oidc` |
| SAML ACS (HTTP-POST) | `/sso/instance/saml` | `/sso/servers/<id>/saml` |
| SAML metadata, entity ID | `/sso/instance/saml/metadata` | `/sso/servers/<id>/saml/metadata` |

What's checked:

- OpenID Connect: the code flow with PKCE (S256), `state` and `nonce`; the ID
  token's signature against the provider's JWKS (RS, PS, ES256/384 or EdDSA;
  `none` and shared-secret HS algorithms are refused), its issuer, audience,
  expiry and nonce, with three minutes of clock skew. An explicit
  `email_verified: false` doesn't count as a verified email.
- SAML 2.0: the response or the assertion must be signed (RSA or ECDSA over
  SHA-256 or stronger, exclusive canonicalization, by one of the configured
  certificates). Only the signed element is read, IDs must be unique and there
  must be exactly one assertion, so signature wrapping doesn't work.
  Destination, issuer, audience, NotBefore/NotOnOrAfter (three minutes of
  skew), InResponseTo and the bearer confirmation's recipient are checked, each
  answer works once, and encrypted assertions are refused. The NameID is the
  subject; `email`/`mail` and `displayName`/`name` attributes (or their
  claims-URI forms) fill in the rest.
- The provider's answer lands on the instance, which checks it and sends the
  browser on to `/auth/sso/done` with a one-time code in the URL fragment, so
  it never reaches a server log. The app that started the sign-in finishes it
  with that code and a secret only it holds. Redirects only ever go to the
  instance's own public URL, never one taken from the request. No client
  addresses are kept or logged.
- A server's provider is reached (OIDC discovery, JWKS, token) only over
  https, and never at a private, loopback or link-local address, even through
  DNS. The instance's own provider may be on a private network, since only
  its admins set it.

### Pictures

Avatars, banners and server icons can be uploaded to the instance itself. The
app crops them in the browser and saves them as small WebP files (GIFs go up as
they are, so they keep moving). The file is checked to really be a PNG, JPEG,
GIF, WebP or AVIF picture, and it loses what it says about where and how it
was taken (EXIF with a photo's GPS position, XMP, text and comments; colour
profiles and animation stay; an AVIF's are zeroed where they are), then it's
served at `<FUWA_PUBLIC_URL>/media/<id>` to
anyone with the link, so set `FUWA_PUBLIC_URL` to the address people use before
anyone uploads. Pictures aren't encrypted by `FUWA_ENCRYPTION_KEY`, since
they're public at their links. A picture that gets replaced, or that nothing
uses a day after it was uploaded, is deleted, and so are an account's pictures
when the account is deleted.
Pictures kept before an update that takes out more are gone over once, in the
background after start-up (a `.pictures-cleaned-1` file in the data folder says
it's done), and sent to the replica again.

To upload one yourself, call `MediaService.CreateUpload` with the picture's type
and size, then `PUT` the file to the `upload_url` it returns (within ten
minutes, once) and set the returned `media.url` as the avatar, banner or icon.

### Webhooks

A server's managers (anyone with Manage webhooks) make webhooks under Server
settings, Integrations. Each one is an address that posts into one channel,
with no account: CI results, feeds, alerts. Posts are a JSON `POST` shaped like
Discord's, so tools made for Discord webhooks work unchanged:

```sh
curl -X POST "$WEBHOOK_URL" -H 'content-type: application/json' \
  -d '{"content": "Build **passed**", "username": "CI", "embeds": [{"title": "main", "color": 5763719}]}'
```

The address is `<FUWA_PUBLIC_URL>/webhooks/<server id>/<webhook id>/<token>`,
on the same port as everything else. It answers `204`, or `200` with the
message with `?wait=true`. Each webhook may post 30 messages a minute (then
`429` with `Retry-After`), and never pings @everyone, @here or roles. Anyone
with the address can post, so a leaked one is replaced with New address.

### Agents

Agents are accounts a program drives: bots, assistants, integrations that
need to read as well as post. Anyone signed in makes them under Settings,
Agents (instance admins can limit that to admins, or turn it off, with
`FUWA_AGENT_CREATION`), up to 25 each. Each one gets a token, shown once,
that the program sends as `authorization: Bearer <token>` on every call of
the same API the apps use:

```sh
grpcurl -H "authorization: Bearer $AGENT_TOKEN" \
  -d '{"server_id": "…", "channel_id": "…", "content": "Hello! 🤖"}' \
  fuwa.example:443 fuwa.v1.MessageService/SendMessage
```

An agent joins a server only when someone with Manage Server adds it (Server
settings, Integrations, or from the agent's own card). Agents are private
until their owner marks them public: then any server's managers can add them
by username. They talk with the roles they're given, show an AGENT badge,
can't own servers or use direct messages, and go away with the person who
made them.

For JavaScript and TypeScript, the [`@waifu-devs/fuwa`](sdk/) SDK does the
rest: typed clients, commands and mentions, and an event stream that
reconnects and catches up by itself. See [docs/sdk.md](docs/sdk.md).

Agents can be in voice channels too, hearing each person and talking back,
without WebRTC: `CallService.ListenVoice` streams everyone's sound as Opus
frames labelled with who said them, and `SpeakVoice` says frames back. The
[`fuwa-voice`](voice/) crate does both for Rust programs, with a parrot bot
to start from; see [docs/calls.md](docs/calls.md#agents-bots-and-apps). The
TypeScript SDK adds what a spoken conversation needs: each person's
utterances as they speak, speech streamed back as a service makes it, and
stopping when someone talks over it ([docs/sdk.md](docs/sdk.md#conversations)).

### Looking after an instance

Besides the settings above, instance admins get three pages in the app's
instance settings:

- **Accounts**: search every account, make or remove instance admins, give
  someone a new random password (shown once; it signs them out everywhere and
  can also turn off their two-step sign-in), or turn an account off with a
  reason. A turned-off account is signed out and can't sign in until an admin
  turns it back on; its messages and servers stay.
- **Servers**: every server with its owner, usage and caps, whether or not
  you're in it. Save a server's whole database as a plain SQLite file, or
  delete it. The saved file is never encrypted, even on an instance with
  `FUWA_ENCRYPTION_KEY`, so keep it somewhere safe.
- **Announcement**: a banner across the top of every client on the instance,
  for news or maintenance. Info and heads-up banners can be closed; urgent ones
  can't. It can come down by itself after a while.

### Data

```
<FUWA_DATA_PATH>/
  node.db              accounts, sessions, the install id
  dms.db               direct messages: devices, public keys and ciphertext only
  servers/<id>.db      one file per community server
  *.db-log             recent commits not yet folded into the file beside it
  deleted/             deleted servers, parked here instead of erased
  exports/             servers being saved as SQLite files; emptied on start
  media/               uploaded pictures, one file per picture (not encrypted)
```

Every file runs in Turso's concurrent-writer mode (MVCC), which keeps recent
commits in the `.db-log` beside it until they're folded in; keep the two
together. Other SQLite tools can't open a file in that mode: stop fuwa and run
`fuwa to-sqlite <file>` to switch it back to plain SQLite first. fuwa switches
it back to concurrent writes the next time it starts. (An encrypted file opens
only in Turso either way.)

To back up, copy the directory (or stop the server and copy single files). To
bring back a deleted server, move its file from `deleted/` into `servers/` as
`<id>.db` and restart; its pictures (icon, emoji, webhooks' pictures) were
deleted with it and don't come back. A server file dropped into `servers/` is picked up at
startup.

### Scaling out

One `fuwa` process runs a whole instance, and that's the right choice until
one machine isn't enough. Then the same binary runs as separate parts, chosen
with `FUWA_ROLE`:

- **Directory** (one): keeps `node.db` (accounts, sessions, settings), `dms.db`
  (direct messages) and the uploaded pictures, and knows which shard holds each
  community server.
- **Shards** (one or more): each keeps some of the community servers' files and
  sends their live events. New servers go to the shard holding the fewest.
- **Media** (one or more, optional): carries calls' sound. Apps reach its
  `FUWA_MEDIA_PORT` directly; the directory and shards reach it on
  `FUWA_MEDIA_URL`. It keeps nothing. Without one, a split instance has no
  calls. See [docs/calls.md](docs/calls.md).
- **Gateways** (one or more): what clients connect to. They keep nothing, serve
  the web app, and pass each call to the directory or to the shard holding its
  server. A client's one live stream can follow servers on several shards; the
  gateway merges them.

Put your public address (and TLS) in front of the gateways, and keep the
directory and shards on a private network. They answer only calls carrying
the cluster key, apart from `/healthz`.

| Variable | For | What it does |
| --- | --- | --- |
| `FUWA_ROLE` | every part | `all` (the default: everything in one process), `gateway`, `directory`, `shard` or `media` |
| `FUWA_CLUSTER_KEY` | every part | A shared secret of 32+ characters (`openssl rand -hex 32`), the same on every part |
| `FUWA_DIRECTORY_URL` | gateways, shards | Where the directory is, like `http://directory:8080` |
| `FUWA_INTERNAL_URL` | shards | Where gateways and the directory reach this shard, like `http://shard-1:8080` |
| `FUWA_MEDIA_URL` | directory, shards | Where the media parts are, like `http://media:8080`, comma-separated; calls are spread over them  (`https://` for one on the internet) |
| `FUWA_MEDIA_KEY` | directory, shards, media | Optional: a separate 32+ character key for calls to the media parts only, so a media part on a host of its own ([docs/self-hosting.md](docs/self-hosting.md#the-media-part-on-a-host-of-its-own)) needs no cluster key |
| `FUWA_SHARD_ID` | shards | The shard's name (a-z, 0-9, `-`, `_`), shown publicly by `/healthz/parts`. Defaults to one made up on first start and kept in its data folder as `shard-id` |
| `FUWA_REGION` | any part | The region it runs in, like `eu` or `us-west` (see [docs/regions.md](docs/regions.md)). The directory's is the home region; unset is the home region |
| `FUWA_REGION_NAME` | any part | The region's name people see, like `Europe`. Common labels have one built in |

The other variables work as above, read by the part that uses them: set
`FUWA_PUBLIC_URL` (the gateways' address), the admin token, accounts, limits
and telemetry on the directory, which shares its settings with every other
part as they change. Give the directory and each shard their own data folder.
If you use `FUWA_ENCRYPTION_KEY`, set the same one on the directory and every
shard, so server files can move between shards. Every part answers `/healthz`.
The gateways also answer `/healthz/parts` for anyone (a status page, say):
whether the directory, each shard and calls are up, with each shard named by
its `FUWA_SHARD_ID`, so don't name shards after hosts or anything private.

```sh
docker network create fuwa
KEY=$(openssl rand -hex 32)
fuwa_part() { docker run -d --network fuwa -e FUWA_CLUSTER_KEY=$KEY "$@" ghcr.io/waifu-devs/fuwa; }
fuwa_part --name directory -v directory:/data -e FUWA_ROLE=directory \
  -e FUWA_PUBLIC_URL=https://chat.example.com
fuwa_part --name shard-1 -v shard-1:/data -e FUWA_ROLE=shard \
  -e FUWA_DIRECTORY_URL=http://directory:8080 -e FUWA_INTERNAL_URL=http://shard-1:8080
fuwa_part --name gateway -p 8080:8080 -e FUWA_ROLE=gateway \
  -e FUWA_DIRECTORY_URL=http://directory:8080
```

Turning a single-process instance into a split one: its data folder becomes
the directory's, and the first shard to start takes its community servers. The
directory sends that shard every file in its `servers/` folder, the shard
checks each copy and starts with them, and the directory moves its own copies
to `handed-over/` (delete that folder once everything looks right). No other
shard gets any of them, and nothing has to be copied by hand, which matters
where each part has a disk of its own (Railway, Kubernetes, Fly.io). fuwa.chat's
Railway config ([`.railway/railway.ts`](.railway/railway.ts)) splits this way
when its `SPLIT` setting is turned on.

With a bucket set (see [Replicating to a bucket](#replicating-to-a-bucket)),
a directory or shard that lost its volume restores itself from it.

#### Restarts and deploys

Restarting a part, say to deploy a new version, interrupts nobody:

- **A shard** stops, then starts again. Meanwhile gateways hold calls for its
  servers and send them on once it's back (if that takes over 30 seconds, the
  call answers "unavailable"). Live streams stay open: the gateway follows the
  shard's servers again from the last event each client got, so nothing is
  missed and the client never sees a break.
- **The directory** stops, then starts again. Gateways hold calls for it the
  same way, and shards wait for it when they need to check who's calling.
  Once back, it answers clients only after every shard has registered again
  (or 30 seconds have passed), so nobody gets a server list with servers
  missing. Live streams don't need it.
- **A media part** tells every app in a call it's restarting; they join
  again on its replacement with the same place, so nobody sees anyone leave
  and the sound is back within a second or two (see
  [docs/calls.md](docs/calls.md#restarts)). The same happens with one process.
- **A gateway** keeps nothing, so run two or more and start new ones before
  stopping old ones. When it stops, it tells each open live stream to follow
  again; clients do, on another gateway, from where they got to. Its health
  check (`/healthz`) only answers once it has reached the directory, so a new
  gateway doesn't take traffic before it can serve it.

Every part finishes the calls it's answering before it exits on SIGTERM, so
give it up to 45 seconds. A part with its files on a disk of its own (the
directory, shards) can't overlap its old and new processes; a gateway can, and
that's what keeps clients connected.

A shard that stays down is different: after the 30 seconds its servers answer
"unavailable" and live streams following them end, so clients reconnect once
it's back; everything else keeps working. Deleting an account and exporting
someone's data are refused until every shard is up, so nothing is left out. An
instance admin can move a server to another region from the instance's Servers
page, with a pause of a moment and nothing to stop (see
[docs/regions.md](docs/regions.md)). To move a server to another shard by hand,
stop both, move its files (`<id>.db` and any `<id>.db-log` or `-wal` beside
it) from one `servers/` folder to the other, and start them; the directory
learns where it went when the shard starts. Shards check sessions with the directory and remember the answer for a
few seconds, so a signed-out device can keep reading for up to five seconds
(its live streams end at the next heartbeat check).

### Replicating to a bucket

A split instance's directory and shards can copy their files to a bucket as
they change: each file's recent commits go up every second, so losing a
volume loses about a second of writes at most, and a clean shutdown ships
everything. Any S3-compatible bucket works (Railway, Cloudflare R2, AWS S3,
MinIO). With `FUWA_ENCRYPTION_KEY` set, the bucket holds only encrypted data.
An instance run as one process doesn't replicate: it refuses these settings,
and its backups are a copy of its data folder.

| Variable | Default | What it does |
| --- | --- | --- |
| `FUWA_S3_BUCKET` | unset | The bucket |
| `FUWA_S3_ENDPOINT` | AWS, from the region | Its endpoint, like `https://t3.storageapi.dev` on Railway |
| `FUWA_S3_REGION` | `auto` | Its region (`auto` on Railway, R2 and Tigris) |
| `FUWA_S3_ACCESS_KEY_ID`, `FUWA_S3_SECRET_ACCESS_KEY` | unset | Its credentials |
| `FUWA_S3_PATH_STYLE` | `off` | `on` for services that want `endpoint/bucket/key` addresses (MinIO, Garage) |
| `FUWA_S3_PREFIX` | none | A folder in the bucket, to share one between instances |
| `FUWA_REPLICA_PATH` | unset | A folder to replicate to instead (another disk, a network share) |
| `FUWA_REPLICA_INTERVAL` | `1s` | How often new commits go up |
| `FUWA_RESTORE` | `off` | `if-empty`: at start, restore from the replica when the data folder is empty |

Give the directory and every shard the same bucket and prefix (gateways
ignore it): the directory keeps `node.db` and the pictures there, each shard
its servers, under its `FUWA_SHARD_ID`. A part that lost its volume comes
back by starting on an empty one with the same bucket, the same encryption key
and `FUWA_RESTORE=if-empty`: the directory restores `node.db` and the
pictures, a shard the servers it held (so set `FUWA_SHARD_ID`; a made-up name
goes with the volume). Without `FUWA_RESTORE`, a part with an empty data
folder over a bucket that has its files won't start, so a lost volume never
turns into an empty instance quietly writing over its own backup. By hand,
with the part stopped: `fuwa restore [DIR]` restores what the part keeps,
`fuwa restore --server <id> [DIR]` one community server, and
`fuwa restore --list` shows what the bucket holds. How it works, what it
costs and where it's going are in [docs/storage.md](docs/storage.md).

### The anonymous usage signal

Once a day (the first one about five minutes after start), the server sends a
small JSON document to `FUWA_TELEMETRY_URL` so Waifu Devs can see how fuwa is
used. Turn it off with `FUWA_TELEMETRY=off` or `DO_NOT_TRACK=1`. Each signal is
also written to the log, so you can see exactly what left. It contains:

- `schema` (`fuwa.signal.v1`), `sent_at`, `version`, `os`, `arch`, `uptime_seconds`
- `hosting`: `self_hosted`, unless this is Waifu Devs' own hosted instance
- `install_id`: a random id made once per instance, so consecutive signals can be
  told apart from other instances' signals. It says nothing about you.
- `config`: whether standalone accounts are open, closed or off; whether linked
  accounts are on; who can create servers; whether encryption and limits are set
- `totals`: counts of accounts (total, active in the last day and 30 days),
  servers, members, channels, messages (stored and ever sent), message bytes,
  attachments, events and storage bytes

It never contains message content, usernames, account or server ids, server
names, or addresses.

### Anonymous health reports

So we can find bugs and slow spots, each part of the instance also counts
what went wrong and how long things took, and once an hour (and as it shuts
down) sends the counts to `FUWA_REPORTS_URL`, when there's anything to send.
The same switch turns it off. Each report is logged at debug level
(`FUWA_LOG=debug`), so you can see exactly what left. It contains:

- `schema` (`fuwa.report.v1`), a random `report_id`, `since` and `sent_at`,
  `hosting`, `part` (`all`, `directory`, `shard` or `gateway`), the
  `install_id` on the parts that keep accounts, and `bounds_ms`, the timing
  buckets' limits
- `errors`: kinds of failure (a panic, a request that failed on the server's
  side) and where in fuwa's code they happened (a source file and line, or the
  gRPC method), with how many times
- `timings`: how long requests took per gRPC method, starting up and catching
  up an event stream, as counts per bucket
- `usage`: how many times a few features were used
- each entry's app (`server`, `web` or `desktop`), version, platform and OS
  family

The web and desktop apps send their own counts of the same kinds (uncaught
errors, startup, catching up, calls connecting, slow frames, requests, a few
features) to the instance they're signed in to (`NodeService.SendReport`),
which adds them to its report; apps never send anything anywhere else. Each
app has its own "Help fix bugs" switch, on by default, and sends nothing while
the instance's switch is off. Reports never contain message content, names,
file names, ids of people or servers, links or addresses, and who sent an
app's report is never kept.

## Protocol

The API is gRPC, defined in [`proto/fuwa/v1`](proto/fuwa/v1). Browsers call it
as gRPC-Web on the same port. Reflection is on, so you can explore with
[grpcurl](https://github.com/fullstorydev/grpcurl):

```sh
grpcurl -plaintext localhost:8080 list
grpcurl -plaintext -d '{"username":"juan","password":"correct horse battery"}' \
  localhost:8080 fuwa.v1.AuthService/SignUp
grpcurl -plaintext -H "authorization: Bearer $TOKEN" -d '{"name":"Waifu Devs"}' \
  localhost:8080 fuwa.v1.ServerService/CreateServer
```

Sign up or sign in to get a session token, then send it as
`authorization: Bearer <token>` on every call. `EventService.Subscribe` streams
events from any number of your servers on the instance at once. To stay in
sync, subscribe first, wait for the `ready` message, then load each server's
channels and members and apply the events that follow. When the stream drops,
subscribe again with each server's last sequence and nothing is missed.

## Development

```sh
cargo test                     # unit and end-to-end tests
cargo clippy --all-targets     # lints
npx @bufbuild/buf lint         # protocol lints
```

The web app lives in [`web/`](web) (React, TanStack Router, Effect, shadcn/ui and
Animate UI) and talks gRPC-Web to any fuwa instance. With a server running on
port 8080:

```sh
cd web
pnpm install
pnpm wasm                      # build the direct messages' encryption (e2ee-wasm)
pnpm dev                       # the app on http://localhost:5173
pnpm generate                  # regenerate src/gen after changing proto/
pnpm build                     # typecheck and build web/dist
```

`pnpm wasm` needs the WebAssembly target and the wasm-bindgen CLI at the version
`e2ee-wasm/Cargo.toml` pins (`rustup target add wasm32-unknown-unknown` and
`cargo install wasm-bindgen-cli --version 0.2.129 --locked`); run it again
after changing `e2ee/` or `e2ee-wasm/`.

In development Vite passes the app's own API calls to `FUWA_DEV_URL` (default
`http://localhost:8080`), so the dev page works as an instance of its own.
After `pnpm build`, `cargo test --features web` also tests the embedded app.

The desktop app lives in [`desktop/`](desktop), a Cargo workspace of its own
(Rust and GPUI Kit). It talks gRPC-Web to any instance, like the web app:

```sh
cd desktop
cargo run                      # the app (keeps its data where your OS keeps app data)
cargo test                     # its core against an in-process instance
```

On Linux it needs the usual GPUI libraries (`libxkbcommon-dev`,
`libxkbcommon-x11-dev`, `libwayland-dev`, `libvulkan-dev`, `libx11-xcb-dev`,
`libfontconfig-dev`). `FUWA_DESKTOP_HOME=<folder>` keeps its instances, settings
and encrypted messages in one folder instead, to run a second copy signed in as
someone else. Releases attach an installer for each system (`.deb` and
AppImage, `.dmg`, `-setup.exe`) and the bare `fuwa-desktop` program; to make
the installers yourself, `cargo install cargo-packager --locked` and then
`cargo build --release && cargo packager --release` in `desktop/`.

The TypeScript SDK lives in [`sdk/`](sdk), generated from the same protocol;
[docs/sdk.md](docs/sdk.md#working-on-the-sdk) has its commands.

See [AGENTS.md](AGENTS.md) for how the code is laid out.

## Infrastructure

[fuwa.chat](https://fuwa.chat), the instance Waifu Devs hosts, runs in a Railway
project of its own, and that project is public, so anyone can check out the
live infrastructure behind it at
<https://railway.com/project/380ce29d-aa58-46a3-b54f-b8e571dd8702>. It's what
[`.railway/railway.ts`](.railway/railway.ts) declares: the `fuwa` service,
running the image every merge to master publishes, and its `fuwa-data` volume.
