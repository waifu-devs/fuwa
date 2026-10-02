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
- **Usage tracked, limits optional.** Every server counts its members, channels,
  messages and storage. Limits are off unless the operator sets them.
- **Live by design.** Every change is an event in the server's log; clients
  follow the log live and catch up from their last sequence after a reconnect.
- **One app for every instance.** Each instance serves the fuwa web app at its
  own address. The app keeps a list of the instances you've added, hosted or
  self-hosted, and shows all their servers side by side.
- **One binary, split when you need to.** By default one process does
  everything. The same binary can run as gateways, a directory and shards
  instead, to spread a big instance across machines (see
  [Scaling out](#scaling-out)).

## Self-host

```sh
docker run -d --name fuwa -p 8080:8080 -v fuwa:/data \
  -e FUWA_PUBLIC_URL=https://chat.example.com \
  ghcr.io/waifu-devs/fuwa
```

Then open the address in a browser: the web app is served from the same port.

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
| `FUWA_HOST` | `0.0.0.0` | Address to listen on |
| `FUWA_PORT` | `PORT`, else `8080` | Port to listen on |
| `FUWA_PUBLIC_URL` | `http://localhost:<port>` | The URL clients reach this instance on; uploaded pictures are linked through it |
| `FUWA_NODE_NAME` | `Fuwa` | The instance's display name |
| `FUWA_ALLOWED_ORIGINS` | `*` | Browser origins allowed to call the API, comma-separated |
| `FUWA_WEB` | `on` | Serve the web app at `/`; `off` leaves only the API |
| `FUWA_LOCAL_ACCOUNTS` | `open` | Standalone accounts: `open` (anyone can sign up), `closed` (existing accounts only), `off` |
| `FUWA_LINKED_ACCOUNTS` | `open` | Signing in with waifu.dev: `open` (anyone with a waifu.dev account gets one here), `closed` (existing linked accounts only), `off` |
| `FUWA_LINKED_ISSUER` | `https://api.waifu.dev` | The OpenAuth issuer linked accounts sign in with |
| `FUWA_SERVER_CREATION` | `everyone` | Who can create servers: `everyone`, `admins`, `off` |
| `FUWA_ADMIN_TOKEN` | unset | A bearer token with instance-admin rights, for scripts or a control plane (32+ characters) |
| `FUWA_ENCRYPTION_KEY` | unset | 64 hex characters (`openssl rand -hex 32`); encrypts every database at rest |
| `FUWA_LIMIT_SERVERS_PER_ACCOUNT` | unlimited | Servers one account may own |
| `FUWA_LIMIT_MEMBERS` | unlimited | Members per server |
| `FUWA_LIMIT_CHANNELS` | unlimited | Channels per server |
| `FUWA_LIMIT_STORAGE` | unlimited | Database size per server, like `500MB` or `2GiB` |
| `FUWA_LIMIT_ATTACHMENT_STORAGE` | unlimited | Uploaded files per server |
| `FUWA_LIMIT_PICTURE_UPLOAD` | unlimited | Largest avatar, banner or server icon one upload may be, like `8MB` |
| `FUWA_TELEMETRY` | `on` | The anonymous usage signal; `off` turns it off (so does `DO_NOT_TRACK=1`) |
| `FUWA_TELEMETRY_URL` | `https://analytics.waifu.dev/v1/fuwa/signals` | Where the signal goes |
| `FUWA_HOSTING` | `self_hosted` | `hosted` only on Waifu Devs' own instance; reported in the signal |
| `FUWA_LOG` | `info,turso_core=warn` | Log filter ([syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)) |

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
needs waifu.dev sign-in working first, and the other way around, so there's
always a way in.

### Pictures

Avatars, banners and server icons can be uploaded to the instance itself. The
app crops them in the browser and saves them as small WebP files (GIFs go up as
they are, so they keep moving). The file is checked to really be a PNG, JPEG,
GIF, WebP or AVIF picture, then served at `<FUWA_PUBLIC_URL>/media/<id>` to
anyone with the link, so set `FUWA_PUBLIC_URL` to the address people use before
anyone uploads. Pictures aren't encrypted by `FUWA_ENCRYPTION_KEY`, since
they're public at their links. A picture that gets replaced, or that nothing
uses a day after it was uploaded, is deleted, and so are an account's pictures
when the account is deleted.

To upload one yourself, call `MediaService.CreateUpload` with the picture's type
and size, then `PUT` the file to the `upload_url` it returns (within ten
minutes, once) and set the returned `media.url` as the avatar, banner or icon.

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
`<id>.db` and restart. A server file dropped into `servers/` is picked up at
startup.

### Scaling out

One `fuwa` process runs a whole instance, and that's the right choice until
one machine isn't enough. Then the same binary runs as separate parts, chosen
with `FUWA_ROLE`:

- **Directory** (one): keeps `node.db` (accounts, sessions, settings) and the
  uploaded pictures, and knows which shard holds each community server.
- **Shards** (one or more): each keeps some of the community servers' files and
  sends their live events. New servers go to the shard holding the fewest.
- **Gateways** (one or more): what clients connect to. They keep nothing, serve
  the web app, and pass each call to the directory or to the shard holding its
  server. A client's one live stream can follow servers on several shards; the
  gateway merges them.

Put your public address (and TLS) in front of the gateways, and keep the
directory and shards on a private network. They answer only calls carrying
the cluster key, apart from `/healthz`.

| Variable | For | What it does |
| --- | --- | --- |
| `FUWA_ROLE` | every part | `all` (the default: everything in one process), `gateway`, `directory` or `shard` |
| `FUWA_CLUSTER_KEY` | every part | A shared secret of 32+ characters (`openssl rand -hex 32`), the same on every part |
| `FUWA_DIRECTORY_URL` | gateways, shards | Where the directory is, like `http://directory:8080` |
| `FUWA_INTERNAL_URL` | shards | Where gateways and the directory reach this shard, like `http://shard-1:8080` |
| `FUWA_SHARD_ID` | shards | The shard's name (a-z, 0-9, `-`, `_`). Defaults to one made up on first start and kept in its data folder as `shard-id` |

The other variables work as above, read by the part that uses them: set
`FUWA_PUBLIC_URL` (the gateways' address), the admin token, accounts, limits
and telemetry on the directory, which shares its settings with every other
part as they change. Give the directory and each shard their own data folder.
If you use `FUWA_ENCRYPTION_KEY`, set the same one on the directory and every
shard, so server files can move between shards. Every part answers `/healthz`.

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
the directory's (its `servers/` folder can stay there and be served by a shard
started on that same folder, or be moved to shards' folders).

While a shard is down, its servers answer "unavailable" and live streams
following them end, so clients reconnect once it's back; everything else keeps
working. Deleting an account and exporting someone's data are refused until
every shard is up, so nothing is left out. To move a server to another shard,
stop both, move its files (`<id>.db` and any `<id>.db-log` or `-wal` beside
it) from one `servers/` folder to the other, and start them; the directory
learns where it went when the shard starts. Shards check sessions with the directory and remember the answer for a
few seconds, so a signed-out device can keep reading for up to five seconds
(its live streams end at the next heartbeat check).

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
pnpm dev                       # the app on http://localhost:5173
pnpm generate                  # regenerate src/gen after changing proto/
pnpm build                     # typecheck and build web/dist
```

In development Vite passes the app's own API calls to `FUWA_DEV_URL` (default
`http://localhost:8080`), so the dev page works as an instance of its own.
After `pnpm build`, `cargo test --features web` also tests the embedded app.

See [AGENTS.md](AGENTS.md) for how the code is laid out.
