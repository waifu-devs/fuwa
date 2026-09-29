# fuwa

A self-hostable, Discord-like chat server. Run it yourself with one command, or
use the instance Waifu Devs hosts; clients connect to as many instances as they
like, hosted or self-hosted, over the same protocol.

- **One database per community server.** Every server lives in its own
  [Turso](https://github.com/tursodatabase/turso) (SQLite-compatible) file, so a
  server can be backed up, moved or inspected by copying one file.
- **Accounts your way.** Standalone accounts (a username and password kept on the
  instance) work with no dependency on anyone. Linked accounts, signed in through
  waifu.dev, are coming next. The operator switches each kind on or off.
- **Usage tracked, limits optional.** Every server counts its members, channels,
  messages and storage. Limits are off unless the operator sets them.
- **Live by design.** Every change is an event in the server's log; clients
  follow the log live and catch up from their last sequence after a reconnect.

## Self-host

```sh
docker run -d --name fuwa -p 8080:8080 -v fuwa:/data \
  -e FUWA_PUBLIC_URL=https://chat.example.com \
  ghcr.io/waifu-devs/fuwa
```

Or build and run the binary:

```sh
cargo run --release --bin fuwa
```

The first account to sign up becomes the instance's admin. Put it behind a
reverse proxy with TLS for anything public; the server speaks plain HTTP/1.1
and HTTP/2.

### Configuration

Everything is set with environment variables (a `.env` file in the working
directory also works; real environment variables win).

| Variable | Default | What it does |
| --- | --- | --- |
| `FUWA_DATA_PATH` | `~/.fuwa` (`/data` in Docker) | Where the databases live |
| `FUWA_HOST` | `0.0.0.0` | Address to listen on |
| `FUWA_PORT` | `PORT`, else `8080` | Port to listen on |
| `FUWA_PUBLIC_URL` | `http://localhost:<port>` | The URL clients reach this instance on |
| `FUWA_NODE_NAME` | `Fuwa` | The instance's display name |
| `FUWA_ALLOWED_ORIGINS` | `*` | Browser origins allowed to call the API, comma-separated |
| `FUWA_LOCAL_ACCOUNTS` | `open` | Standalone accounts: `open` (anyone can sign up), `closed` (existing accounts only), `off` |
| `FUWA_SERVER_CREATION` | `everyone` | Who can create servers: `everyone`, `admins`, `off` |
| `FUWA_ADMIN_TOKEN` | unset | A bearer token with instance-admin rights, for scripts or a control plane (32+ characters) |
| `FUWA_ENCRYPTION_KEY` | unset | 64 hex characters (`openssl rand -hex 32`); encrypts every database at rest |
| `FUWA_LIMIT_SERVERS_PER_ACCOUNT` | unlimited | Servers one account may own |
| `FUWA_LIMIT_MEMBERS` | unlimited | Members per server |
| `FUWA_LIMIT_CHANNELS` | unlimited | Channels per server |
| `FUWA_LIMIT_STORAGE` | unlimited | Database size per server, like `500MB` or `2GiB` |
| `FUWA_LIMIT_ATTACHMENT_STORAGE` | unlimited | Uploaded files per server |
| `FUWA_TELEMETRY` | `on` | The anonymous usage signal; `off` turns it off (so does `DO_NOT_TRACK=1`) |
| `FUWA_TELEMETRY_URL` | `https://analytics.waifu.dev/v1/fuwa/signals` | Where the signal goes |
| `FUWA_HOSTING` | `self_hosted` | `hosted` only on Waifu Devs' own instance; reported in the signal |
| `FUWA_LOG` | `info,turso_core=warn` | Log filter ([syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)) |

The `FUWA_LIMIT_*` values are instance-wide defaults. An admin can give a single
server its own caps with `AdminService.SetServerLimits`; a server's own caps win
over the defaults.

### Data

```
<FUWA_DATA_PATH>/
  node.db              accounts, sessions, the install id
  servers/<id>.db      one file per community server
  deleted/             deleted servers, parked here instead of erased
```

To back up, copy the directory (or stop the server and copy single files). To
bring back a deleted server, move its file from `deleted/` into `servers/` as
`<id>.db` and restart. A server file dropped into `servers/` is picked up at
startup.

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

See [AGENTS.md](AGENTS.md) for how the code is laid out.
